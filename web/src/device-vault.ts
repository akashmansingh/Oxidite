/**
 * Browser device vault providing local PIN-encrypted credential storage via IndexedDB and Web Crypto.
 *
 * Invariant: Plaintext PINs or unencrypted device credentials are NEVER stored in browser
 * storage (no localStorage, no sessionStorage fallback), and PINs are NEVER transmitted over the wire.
 */

export interface EnrolledDeviceRecord {
  deviceId: string;
  label: string;
  salt: Uint8Array;
  iv: Uint8Array;
  ciphertext: Uint8Array;
}

const DB_NAME = 'oxidite_vault';
const STORE_NAME = 'devices';
const KEY_CURRENT_DEVICE = 'current_device';
const PBKDF2_ITERATIONS = 100_000;

export class DeviceVault {
  /**
   * Checks whether IndexedDB is available in the current browser context.
   */
  static isAvailable(): boolean {
    return typeof indexedDB !== 'undefined' && indexedDB !== null;
  }

  private static getDb(): Promise<IDBDatabase> {
    if (!this.isAvailable()) {
      return Promise.reject(new Error('IndexedDB is not available; unencrypted fallback is forbidden'));
    }

    return new Promise((resolve, reject) => {
      const request = indexedDB.open(DB_NAME, 1);

      request.onupgradeneeded = () => {
        const db = request.result;
        if (!db.objectStoreNames.contains(STORE_NAME)) {
          db.createObjectStore(STORE_NAME);
        }
      };

      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error ?? new Error('Failed to open device vault database'));
    });
  }

  /**
   * Returns true if a device is currently enrolled in the vault.
   */
  static async hasEnrolledDevice(): Promise<boolean> {
    if (!this.isAvailable()) return false;
    try {
      const db = await this.getDb();
      return new Promise((resolve) => {
        const tx = db.transaction([STORE_NAME], 'readonly');
        const store = tx.objectStore(STORE_NAME);
        const req = store.get(KEY_CURRENT_DEVICE);
        req.onsuccess = () => resolve(req.result != null);
        req.onerror = () => resolve(false);
      });
    } catch {
      return false;
    }
  }

  /**
   * Returns the device ID and non-secret label of the enrolled device, or null.
   */
  static async getEnrolledInfo(): Promise<{ deviceId: string; label: string } | null> {
    if (!this.isAvailable()) return null;
    try {
      const db = await this.getDb();
      return new Promise((resolve) => {
        const tx = db.transaction([STORE_NAME], 'readonly');
        const store = tx.objectStore(STORE_NAME);
        const req = store.get(KEY_CURRENT_DEVICE);
        req.onsuccess = () => {
          if (req.result) {
            resolve({
              deviceId: req.result.deviceId,
              label: req.result.label,
            });
          } else {
            resolve(null);
          }
        };
        req.onerror = () => resolve(null);
      });
    } catch {
      return null;
    }
  }

  /**
   * Derives an AES-GCM wrapping key using PBKDF2-SHA-256 with the user PIN and salt.
   */
  private static async deriveKey(pin: string, salt: Uint8Array): Promise<CryptoKey> {
    const enc = new TextEncoder();
    const pinKey = await crypto.subtle.importKey(
      'raw',
      enc.encode(pin),
      { name: 'PBKDF2' },
      false,
      ['deriveKey']
    );

    return crypto.subtle.deriveKey(
      {
        name: 'PBKDF2',
        salt: salt as any,
        iterations: PBKDF2_ITERATIONS,
        hash: 'SHA-256',
      },
      pinKey,
      { name: 'AES-GCM', length: 256 },
      false,
      ['encrypt', 'decrypt']
    );
  }

  /**
   * Enrolls a device by encrypting the server credential using AES-GCM and saving it to IndexedDB.
   */
  static async enroll(
    deviceId: string,
    token: string,
    label: string,
    pin: string
  ): Promise<void> {
    if (!this.isAvailable()) {
      throw new Error('IndexedDB is not available; unencrypted fallback is forbidden');
    }

    if (!pin || pin.length < 6) {
      throw new Error('PIN must be at least 6 characters');
    }

    const salt = new Uint8Array(16);
    crypto.getRandomValues(salt);

    const iv = new Uint8Array(12);
    crypto.getRandomValues(iv);

    const key = await this.deriveKey(pin, salt);
    const enc = new TextEncoder();
    const ciphertextBuffer = await crypto.subtle.encrypt(
      { name: 'AES-GCM', iv: iv as any },
      key,
      enc.encode(token)
    );

    const record: EnrolledDeviceRecord = {
      deviceId,
      label,
      salt,
      iv,
      ciphertext: new Uint8Array(ciphertextBuffer),
    };

    const db = await this.getDb();
    return new Promise((resolve, reject) => {
      const tx = db.transaction([STORE_NAME], 'readwrite');
      const store = tx.objectStore(STORE_NAME);
      const req = store.put(record, KEY_CURRENT_DEVICE);
      req.onsuccess = () => resolve();
      req.onerror = () => reject(req.error ?? new Error('Failed to save enrolled device in vault'));
    });
  }

  /**
   * Unlocks the device credential using the user PIN.
   * Throws an error if the PIN is incorrect or decryption fails.
   */
  static async unlock(pin: string): Promise<string> {
    if (!this.isAvailable()) {
      throw new Error('IndexedDB is not available; unencrypted fallback is forbidden');
    }

    const db = await this.getDb();
    const record: EnrolledDeviceRecord = await new Promise((resolve, reject) => {
      const tx = db.transaction([STORE_NAME], 'readonly');
      const store = tx.objectStore(STORE_NAME);
      const req = store.get(KEY_CURRENT_DEVICE);
      req.onsuccess = () => {
        if (req.result) resolve(req.result);
        else reject(new Error('No enrolled device found'));
      };
      req.onerror = () => reject(req.error ?? new Error('Failed to read device from vault'));
    });

    try {
      const key = await this.deriveKey(pin, record.salt);
      const decryptedBuffer = await crypto.subtle.decrypt(
        { name: 'AES-GCM', iv: record.iv as any },
        key,
        record.ciphertext as any
      );

      return new TextDecoder().decode(decryptedBuffer);
    } catch {
      throw new Error('Incorrect PIN or corrupted device vault');
    }
  }

  /**
   * Forgets the enrolled device by removing its record from IndexedDB.
   */
  static async forget(): Promise<void> {
    if (!this.isAvailable()) return;
    try {
      const db = await this.getDb();
      return new Promise((resolve, reject) => {
        const tx = db.transaction([STORE_NAME], 'readwrite');
        const store = tx.objectStore(STORE_NAME);
        const req = store.delete(KEY_CURRENT_DEVICE);
        req.onsuccess = () => resolve();
        req.onerror = () => reject(req.error ?? new Error('Failed to delete device from vault'));
      });
    } catch {
      // Ignored if DB is already gone or cannot open
    }
  }
}
