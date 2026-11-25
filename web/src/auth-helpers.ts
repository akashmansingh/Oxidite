import type { SshAuthSpec } from "./protocol";

/** Extracts a single-use pairing grant from `#pair=...` in the URL hash. */
export function parsePairGrantFromHash(hash: string): string | null {
  const clean = hash.startsWith("#") ? hash.slice(1) : hash;
  if (!clean) return null;
  const params = new URLSearchParams(clean);
  const pair = params.get("pair")?.trim();
  return pair ? pair : null;
}

/** Builds a fragment-based pairing URL so the single-use grant stays out of HTTP logs. */
export function buildPairShareUrl(origin: string, pathname: string, grant: string): string {
  const cleanPath = pathname || "/";
  return `${origin}${cleanPath}#pair=${encodeURIComponent(grant)}`;
}

/** Picks a human-readable default label for the current browser/device. */
export function defaultDeviceLabel(userAgent = typeof navigator !== "undefined" ? navigator.userAgent : ""): string {
  const ua = userAgent.toLowerCase();
  if (ua.includes("iphone")) return "iPhone Browser";
  if (ua.includes("ipad")) return "iPad Browser";
  if (ua.includes("android")) return "Android Browser";
  if (ua.includes("windows")) return "Windows Browser";
  if (ua.includes("mac os") || ua.includes("macintosh")) return "macOS Browser";
  if (ua.includes("linux")) return "Linux Browser";
  return "Browser Device";
}

/** Builds an SSH authentication variant matching `oxidite_proto::AuthSpec`. */
export function buildSshAuthSpec(
  kind: string,
  password: string,
  keyPath: string,
  passphrase: string,
): SshAuthSpec {
  if (kind === "publickey") {
    return {
      PublicKey: {
        key_path: keyPath.trim(),
        passphrase: passphrase ? passphrase : null,
      },
    };
  }
  if (kind === "interactive") {
    return "Interactive";
  }
  return { Password: password };
}

export type GatewayErrorKind = "unauthorized" | "offline" | "server_error";

export function classifyGatewayStatus(status: number | null): GatewayErrorKind {
  if (status === 401 || status === 403) return "unauthorized";
  if (status === null) return "offline";
  return "server_error";
}

export function computeRefreshDelayMs(failures: number): number {
  if (failures <= 0) return 5000;
  return Math.min(15_000, 2000 * 2 ** Math.min(failures - 1, 3));
}
