// Wire format: docs/contracts/wire-protocol.md.
// Server must use rmp_serde::to_vec_named, serde's default externally tagged enums,
// and serde_bytes for byte fields so they arrive as msgpack `bin`.
import { encode, decode } from "@msgpack/msgpack";

export const PROTOCOL_VERSION = 1;
export type TermSize = { rows: number; cols: number };
export interface ShellCapability {
  id: string;
  available: boolean;
}
export type SshAuthSpec =
  | { Password: string }
  | { PublicKey: { key_path: string; passphrase: string | null } }
  | "Interactive";
export type BackendSpec =
  | { LocalPty: { shell: string | null; cwd: string | null } }
  | { Ssh: { host: string; port: number; user: string; auth: SshAuthSpec; multiplexer: "None" | "Tmux" } };
export type SessionState =
  | "Starting" | "Running" | "Degraded" | "Detached"
  | { Reconnecting: { attempt: number } } | { Exited: { code: number | null } } | { Failed: { reason: string } };
export type Msg = { t: string; [k: string]: unknown };

/** Encode a client message: unit variants are bare strings, others `{ Name: body }`. */
export const pack = (name: string, body?: unknown): Uint8Array => encode(body === undefined ? name : { [name]: body });

/** Flatten an externally tagged server message into `{ t, ...fields }`. */
export function parse(buf: ArrayBuffer): Msg {
  const raw = decode(buf);
  if (typeof raw === "string") return { t: raw };
  const [t, v] = Object.entries(raw as Record<string, unknown>)[0] ?? ["", null];
  const plain = typeof v === "object" && v !== null && !Array.isArray(v) && !(v instanceof Uint8Array);
  return plain ? { t, ...(v as object) } : { t, v };
}
export const bytesOf = (x: unknown): Uint8Array => (x instanceof Uint8Array ? x : Uint8Array.from(x as number[]));
export const stateName = (s: SessionState): string => (typeof s === "string" ? s : Object.keys(s)[0] ?? "");

const hex = (b: number): string => b.toString(16).padStart(2, "0");
/** Session ids are 16 raw bytes on the wire but hyphenated strings in REST. The UI always holds the string form. */
export function idStr(x: unknown): string {
  if (typeof x === "string") return x;
  const h = Array.from(bytesOf(x), hex).join("");
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}
/** The server decodes a session id as 16 bytes and rejects the string form. */
export function idBytes(s: string): Uint8Array {
  const h = s.replace(/-/g, "");
  return Uint8Array.from({ length: 16 }, (_, i) => parseInt(h.slice(i * 2, i * 2 + 2), 16));
}
