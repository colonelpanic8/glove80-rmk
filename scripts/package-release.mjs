#!/usr/bin/env node
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const UF2_MAGIC0 = 0x0a324655;
const UF2_MAGIC1 = 0x9e5d5157;
const UF2_MAGIC_END = 0x0ab16f30;
const UF2_FLAG_FAMILY_ID = 0x00002000;
const APPLICATION_START = 0x00026000;
const APPLICATION_END = 0x000dc000;

function parseArgs(argv) {
  const args = {};
  for (let i = 2; i < argv.length; i += 2) {
    if (!argv[i].startsWith("--") || argv[i + 1] === undefined) {
      throw new Error(`bad argument near ${argv[i] ?? "end of arguments"}`);
    }
    args[argv[i].slice(2)] = argv[i + 1];
  }
  return args;
}

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function hex(value) {
  return `0x${value.toString(16)}`;
}

function inspectUf2(path, expectedFamily) {
  const data = readFileSync(path);
  if (data.length === 0 || data.length % 512 !== 0) {
    throw new Error(`${path} is not a non-empty UF2 block stream`);
  }

  const families = new Set();
  let start = Number.POSITIVE_INFINITY;
  let end = 0;
  const blocks = data.length / 512;
  for (let offset = 0; offset < data.length; offset += 512) {
    const block = offset / 512;
    if (
      data.readUInt32LE(offset) !== UF2_MAGIC0 ||
      data.readUInt32LE(offset + 4) !== UF2_MAGIC1 ||
      data.readUInt32LE(offset + 508) !== UF2_MAGIC_END
    ) {
      throw new Error(`${path} has invalid UF2 magic in block ${block}`);
    }
    const flags = data.readUInt32LE(offset + 8);
    const address = data.readUInt32LE(offset + 12);
    const payloadSize = data.readUInt32LE(offset + 16);
    const blockNumber = data.readUInt32LE(offset + 20);
    const declaredBlocks = data.readUInt32LE(offset + 24);
    if (blockNumber !== block || declaredBlocks !== blocks) {
      throw new Error(`${path} has inconsistent UF2 block numbering at block ${block}`);
    }
    if (flags & UF2_FLAG_FAMILY_ID) families.add(data.readUInt32LE(offset + 28));
    start = Math.min(start, address);
    end = Math.max(end, address + payloadSize);
  }

  if (families.size !== 1 || !families.has(expectedFamily)) {
    throw new Error(`${path} has family IDs ${[...families].map(hex)}, expected ${hex(expectedFamily)}`);
  }
  if (start < APPLICATION_START || end > APPLICATION_END) {
    throw new Error(
      `${path} range ${hex(start)}-${hex(end)} is outside ${hex(APPLICATION_START)}-${hex(APPLICATION_END)}`,
    );
  }
  return { blocks, familyId: hex(expectedFamily), addressStart: hex(start), addressEnd: hex(end) };
}

const args = parseArgs(process.argv);
for (const required of [
  "dist",
  "version",
  "source-commit",
  "dirty",
  "config-commit",
  "config-dirty",
  "rmk-commit",
  "rmk-version",
  "rust-toolchain",
  "protocol-version",
]) {
  if (!args[required]) throw new Error(`missing --${required}`);
}

const halves = [
  { name: "left", suffix: "lh", family: 0x9807b007 },
  { name: "right", suffix: "rh", family: 0x9808b007 },
];
const artifacts = [];
for (const half of halves) {
  const base = `glove80-rmk-${args.version}-${half.suffix}`;
  const uf2Name = `${base}.uf2`;
  const elfName = `${base}.elf`;
  const uf2Path = join(args.dist, uf2Name);
  const elfPath = join(args.dist, elfName);
  artifacts.push({
    half: half.name,
    target: "thumbv7em-none-eabihf",
    uf2: { file: uf2Name, sha256: sha256(uf2Path), ...inspectUf2(uf2Path, half.family) },
    elf: { file: elfName, sha256: sha256(elfPath) },
  });
}

const manifest = {
  schemaVersion: 1,
  project: "glove80-rmk",
  version: args.version,
  source: { commit: args["source-commit"], dirty: args.dirty === "true" },
  configuration:
    args["config-commit"] === "standalone"
      ? null
      : { commit: args["config-commit"], dirty: args["config-dirty"] === "true" },
  rmk: { commit: args["rmk-commit"], version: args["rmk-version"] },
  rustToolchain: args["rust-toolchain"],
  productProtocolVersion: args["protocol-version"],
  applicationRange: { start: hex(APPLICATION_START), end: hex(APPLICATION_END) },
  artifacts,
};

writeFileSync(join(args.dist, "manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);
const checksumLines = artifacts
  .flatMap((artifact) => [artifact.uf2, artifact.elf])
  .map((artifact) => `${artifact.sha256}  ${artifact.file}`)
  .join("\n");
writeFileSync(join(args.dist, "SHA256SUMS"), `${checksumLines}\n`);

for (const artifact of artifacts) {
  console.log(
    `${artifact.half}: ${artifact.uf2.addressStart}-${artifact.uf2.addressEnd}, ` +
      `${artifact.uf2.familyId}, ${artifact.uf2.sha256}`,
  );
}
