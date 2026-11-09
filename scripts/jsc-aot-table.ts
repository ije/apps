import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, join } from "node:path";

const [root, engine, output] = process.argv.slice(2);
process.env.AOT_ENGINE_DIRECTORY = join(engine, "aot");
const { generate } = await import(join(root, "src/codegen/builtins/index.ts"));
const { globAllSources } = await import(join(root, "scripts/glob-sources.ts"));
const { crateOf } = await import(join(root, "scripts/build/codegen.ts"));
const sources = globAllSources().bindgen;
const { table } = await generate({
  bunRoot: root,
  javaScriptCore: engine,
  prebuiltGenerated: null,
  target: "Darwin",
  bindgenSources: sources.map((source: string) => `${crateOf(source)}=${source}`),
  typesOnly: sources.filter((source: string) => source.endsWith("_test.bind.ts")).map((source: string) => basename(source)),
});
mkdirSync(output, { recursive: true });
for (const [name, text] of table) {
  const path = join(output, name);
  if (!existsSync(path) || readFileSync(path, "utf8") !== text) writeFileSync(path, text);
}
