import { describe, expect, test, beforeEach } from 'bun:test';
import { DeviceVault, type EnrolledDeviceRecord } from './device-vault';

// In-memory mock of IndexedDB for testing Web Crypto + IDB integration
class MockIDBDatabase {
  records = new Map<string, any>();
  objectStoreNames = {
    contains: (_name: string) => false,
  };

  createObjectStore(_name: string) {
    this.objectStoreNames.contains = () => true;
  }

  transaction(_storeNames: string[], _mode: string) {
    const db = this;
    return {
      objectStore(_name: string) {
        return {
          get(key: string) {
            const req: any = { result: db.records.get(key) };
            setTimeout(() => req.onsuccess?.({ target: req }), 0);
            return req;
          },
          put(value: any, key: string) {
            db.records.set(key, value);
            const req: any = {};
            setTimeout(() => req.onsuccess?.({ target: req }), 0);
            return req;
          },
          delete(key: string) {
            db.records.delete(key);
            const req: any = {};
            setTimeout(() => req.onsuccess?.({ target: req }), 0);
            return req;
          },
        };
      },
    };
  }
}

class MockIDBFactory {
  db = new MockIDBDatabase();

  open(_name: string, _version?: number) {
    const db = this.db;
    const req: any = { result: db };
    setTimeout(() => {
      req.onupgradeneeded?.({ target: req });
      req.onsuccess?.({ target: req });
    }, 0);
    return req;
  }
}

describe('DeviceVault', () => {
  beforeEach(() => {
    // Reset global storage and mock IndexedDB
    (globalThis as any).localStorage = {
      store: new Map<string, string>(),
      getItem(k: string) { return this.store.get(k) ?? null; },
      setItem(k: string, v: string) { this.store.set(k, v); },
      removeItem(k: string) { this.store.delete(k); },
      clear() { this.store.clear(); },
    };
    (globalThis as any).sessionStorage = {
      store: new Map<string, string>(),
      getItem(k: string) { return this.store.get(k) ?? null; },
      setItem(k: string, v: string) { this.store.set(k, v); },
      removeItem(k: string) { this.store.delete(k); },
      clear() { this.store.clear(); },
    };
    (globalThis as any).indexedDB = new MockIDBFactory();
  });

  test('enrolls and unlocks with correct PIN', async () => {
    const deviceId = '11111111-1111-1111-1111-111111111111';
    const secretToken = 'od1.11111111-1111-1111-1111-111111111111.0123456789abcdef0123456789abcdef';
    const pin = '849201';

    await DeviceVault.enroll(deviceId, secretToken, 'My Work Laptop', pin);

    expect(await DeviceVault.hasEnrolledDevice()).toBe(true);

    const info = await DeviceVault.getEnrolledInfo();
    expect(info?.deviceId).toBe(deviceId);
    expect(info?.label).toBe('My Work Laptop');

    const unlockedToken = await DeviceVault.unlock(pin);
    expect(unlockedToken).toBe(secretToken);
  });

  test('wrong PIN fails unlock', async () => {
    const deviceId = '22222222-2222-2222-2222-222222222222';
    const secretToken = 'od1.22222222-2222-2222-2222-222222222222.aabbccddeeff00112233445566778899';
    const correctPin = '123456';
    const wrongPin = '654321';

    await DeviceVault.enroll(deviceId, secretToken, 'Phone', correctPin);

    await expect(DeviceVault.unlock(wrongPin)).rejects.toThrow();
  });

  test('PIN less than 6 digits is rejected', async () => {
    const deviceId = '33333333-3333-3333-3333-333333333333';
    const secretToken = 'od1.33333333-3333-3333-3333-333333333333.11223344556677889900aabbccddeeff';

    await expect(DeviceVault.enroll(deviceId, secretToken, 'ShortPIN', '12345')).rejects.toThrow(
      /PIN must be at least 6 characters/i
    );
  });

  test('stored ciphertext does not contain plaintext secret or PIN', async () => {
    const deviceId = '44444444-4444-4444-4444-444444444444';
    const rawSecret = 'super_secret_high_entropy_token_bytes_value_1234';
    const secretToken = `od1.${deviceId}.${rawSecret}`;
    const pin = '998877';

    await DeviceVault.enroll(deviceId, secretToken, 'AuditCheck', pin);

    const mockFactory = (globalThis as any).indexedDB as MockIDBFactory;
    const record: EnrolledDeviceRecord = mockFactory.db.records.get('current_device');
    expect(record).toBeDefined();

    expect(record.salt).toBeDefined();
    expect(record.iv).toBeDefined();
    expect(record.salt.length).toBe(16);
    expect(record.iv.length).toBe(12);

    const ciphertextString = new TextDecoder().decode(record.ciphertext);
    expect(ciphertextString).not.toContain(rawSecret);
    expect(ciphertextString).not.toContain(pin);

    // Verify localStorage and sessionStorage contain NO secrets or fallback
    expect((globalThis as any).localStorage.store.size).toBe(0);
    expect((globalThis as any).sessionStorage.store.size).toBe(0);
  });

  test('unavailable indexedDB throws error and never falls back to localStorage or sessionStorage', async () => {
    (globalThis as any).indexedDB = undefined;

    expect(DeviceVault.isAvailable()).toBe(false);

    await expect(
      DeviceVault.enroll('55555555-5555-5555-5555-555555555555', 'od1.test.secret', 'NoIDB', '123456')
    ).rejects.toThrow(/IndexedDB is not available/i);

    expect((globalThis as any).localStorage.store.size).toBe(0);
    expect((globalThis as any).sessionStorage.store.size).toBe(0);
  });

  test('forget operation removes stored device', async () => {
    const deviceId = '66666666-6666-6666-6666-666666666666';
    const secretToken = 'od1.66666666-6666-6666-6666-666666666666.aabbccddee1122334455667788990011';
    const pin = '112233';

    await DeviceVault.enroll(deviceId, secretToken, 'ToForget', pin);
    expect(await DeviceVault.hasEnrolledDevice()).toBe(true);

    await DeviceVault.forget();
    expect(await DeviceVault.hasEnrolledDevice()).toBe(false);
    expect(await DeviceVault.getEnrolledInfo()).toBeNull();
    await expect(DeviceVault.unlock(pin)).rejects.toThrow();
  });
});
