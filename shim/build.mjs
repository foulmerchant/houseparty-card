// Bundles the shim + wasm-bindgen glue into one ES module, ships the .wasm
// beside it, and bakes the .wasm's SHA-256 into the JS so the shim refuses
// to run a swapped or mismatched binary.
import { build } from "esbuild";
import { createHash } from "node:crypto";
import { copyFile, mkdir, readFile } from "node:fs/promises";

const wasmPath = "../pkg/houseparty_card_ui_bg.wasm";
const sha256 = createHash("sha256").update(await readFile(wasmPath)).digest("hex");

await mkdir("../dist", { recursive: true });
await build({
  entryPoints: ["src/index.ts"],
  bundle: true,
  format: "esm",
  target: "es2022", // native #private fields, no down-levelling
  minify: true,
  legalComments: "none",
  define: { __WASM_SHA256__: JSON.stringify(sha256) },
  outfile: "../dist/houseparty-card.js",
});
await copyFile(wasmPath, "../dist/houseparty-card_bg.wasm");
console.log(`built dist/houseparty-card.js + .wasm (sha256 ${sha256})`);
