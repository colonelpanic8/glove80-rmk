#!/usr/bin/env node
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

const expectedRmkCommit = process.argv[2];
if (!expectedRmkCommit) {
  throw new Error("usage: check-rynk-wasm-provenance.mjs <rmk-commit>");
}

const provenancePath = "ui/src/vendor/rynk-wasm/provenance.json";
const provenance = JSON.parse(readFileSync(provenancePath, "utf8"));
const wasm = readFileSync("ui/src/vendor/rynk-wasm/rynk_wasm_bg.wasm");
const checksum = createHash("sha256").update(wasm).digest("hex");

if (provenance.rmkCommit !== expectedRmkCommit) {
  throw new Error(
    `vendored Rynk commit ${provenance.rmkCommit} does not match RMK ${expectedRmkCommit}`,
  );
}
if (provenance.wasmSha256 !== checksum) {
  throw new Error(`vendored Rynk WASM checksum ${checksum} does not match provenance`);
}

console.log(`Rynk WASM provenance: ${expectedRmkCommit} (${checksum})`);
