// Prepares Bun's checkout to build scripts/aot-front-end: Bun's codegen, the Rust half of the AOT schema and built-in
// table, the archive of Bun's built-in sources and the vendored crates. Prints the codegen folder.
import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, join } from "node:path";

const [root, engine] = process.argv.slice(2);
const aot = join(engine, "aot");
const codegen = join(root, "build/release-codegen/codegen");
process.env.AOT_ENGINE_DIRECTORY = aot;
const run = (...args: string[]) => {
  const result = spawnSync(process.execPath, args, { cwd: root, stdio: ["ignore", 2, 2] });
  if (result.status !== 0) throw new Error(`Failed: bun ${args.join(" ")}`);
};

run("install", "--frozen-lockfile");
run("scripts/build.ts", "--profile=release", "--mode=codegen", "--target=codegen");
run(join(aot, "ir/generator/index.ts"), "--schema", join(aot, "ir/AOTSchema.def"), "--rust-out", codegen);

const { generate } = await import(join(root, "src/codegen/builtins/index.ts"));
const { globAllSources } = await import(join(root, "scripts/glob-sources.ts"));
const { crateOf } = await import(join(root, "scripts/build/codegen.ts"));
const sources: string[] = globAllSources().bindgen;
const generated = await generate({
  bunRoot: root,
  javaScriptCore: engine,
  prebuiltGenerated: null,
  target: "Darwin",
  bindgenSources: sources.map(source => `${crateOf(source)}=${source}`),
  typesOnly: sources.filter(source => source.endsWith("_test.bind.ts")).map(source => basename(source)),
});
for (const [name, text] of generated.codegen) writeFileSync(join(codegen, name), text);

run(
  "src/codegen/builtins-manifest.ts",
  "--debug=OFF",
  join(codegen, "builtins.manifest"),
  join(codegen, "builtin_sources.bin"),
  `--engine-functions=${join(codegen, "aot_engine_functions.json")}`,
  `--engine-directory=${aot}`,
  `--bundled-helpers=${join(codegen, "runtime.out.js")}`,
  `--evaluated-modules=${join(codegen, "InternalModuleRegistryConstants.bin")}`,
);

const { allDeps } = await import(join(root, "scripts/build/deps/index.ts"));
for (const name of ["lolhtml", "rust-argon2", "zstd", "boringssl"]) {
  const dep = allDeps.find((dep: { name: string }) => dep.name === name);
  const { repo, commit } = dep.source({});
  const patches = (dep.patches ?? []).map((patch: string) => join(root, patch));
  const cache = join(root, "build/vendor-cache");
  run("scripts/build/fetch-cli.ts", "dep", name, repo, commit, join(root, "vendor", name), cache, ...patches);
}

// WARNING: the crate is a member of Bun's workspace, so it builds with Bun's lockfile, patches and toolchain.
const crate = join(root, "src/apps_aot");
mkdirSync(crate, { recursive: true });
for (const file of ["Cargo.toml", "lib.rs"]) copyFileSync(join(import.meta.dir, "aot-front-end", file), join(crate, file));
const manifest = readFileSync(join(root, "Cargo.toml"), "utf8");
if (!manifest.includes('"src/apps_aot"')) {
  writeFileSync(join(root, "Cargo.toml"), manifest.replace(/^members = \[$/m, 'members = [\n  "src/apps_aot",'));
}
console.log(codegen);
