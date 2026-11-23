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
export type BackendSpec =
  | { LocalPty: { shell: string | null; cwd: string | null } }
  | { Ssh: { host: string; port: number; user: string; auth: { Password: string }; multiplexer: "None" | "Tmux" } };
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
