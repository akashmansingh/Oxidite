import { describe, expect, test } from "bun:test";
import {
  buildPairShareUrl,
  buildSshAuthSpec,
  classifyGatewayStatus,
  computeRefreshDelayMs,
  defaultDeviceLabel,
  parsePairGrantFromHash,
} from "./auth-helpers";

describe("auth-helpers", () => {
  test("parses and builds #pair= grant URLs", () => {
    const grant = "eg_abc123+xyz/9=";
    const url = buildPairShareUrl("https://gw.example:3030", "/", grant);
    expect(url).toBe("https://gw.example:3030/#pair=eg_abc123%2Bxyz%2F9%3D");
    const parsed = parsePairGrantFromHash(new URL(url).hash);
    expect(parsed).toBe(grant);
    expect(parsePairGrantFromHash("")).toBeNull();
    expect(parsePairGrantFromHash("#other=1")).toBeNull();
  });

  test("derives device label by platform", () => {
    expect(defaultDeviceLabel("Mozilla/5.0 (iPhone; CPU iPhone OS 17_0)")).toBe("iPhone Browser");
    expect(defaultDeviceLabel("Mozilla/5.0 (Linux; Android 14)")).toBe("Android Browser");
    expect(defaultDeviceLabel("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")).toBe("Windows Browser");
    expect(defaultDeviceLabel("")).toBe("Browser Device");
  });

  test("builds password, public key, and interactive SSH auth specs", () => {
    expect(buildSshAuthSpec("password", "pw123", "", "")).toEqual({ Password: "pw123" });
    expect(buildSshAuthSpec("publickey", "", "  ~/.ssh/id_ed25519  ", "")).toEqual({
      PublicKey: { key_path: "~/.ssh/id_ed25519", passphrase: null },
    });
    expect(buildSshAuthSpec("publickey", "", "/keys/id", "sec")).toEqual({
      PublicKey: { key_path: "/keys/id", passphrase: "sec" },
    });
    expect(buildSshAuthSpec("interactive", "", "", "")).toBe("Interactive");
  });

  test("classifies unauthorized vs offline vs server error and bounds retry backoff", () => {
    expect(classifyGatewayStatus(401)).toBe("unauthorized");
    expect(classifyGatewayStatus(403)).toBe("unauthorized");
    expect(classifyGatewayStatus(null)).toBe("offline");
    expect(classifyGatewayStatus(502)).toBe("server_error");

    expect(computeRefreshDelayMs(0)).toBe(5000);
    expect(computeRefreshDelayMs(1)).toBe(2000);
    expect(computeRefreshDelayMs(2)).toBe(4000);
    expect(computeRefreshDelayMs(10)).toBe(15_000);
  });
});
