import { catalog, components } from "../src/components/chat/ui/catalog";
import { z } from "zod";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { readFile, writeFile } from "node:fs/promises";
const here = dirname(fileURLToPath(import.meta.url));
const content =
  JSON.stringify(
    {
      version: 1,
      prompt: catalog.prompt(),
      schema: catalog.jsonSchema(),
      components: Object.fromEntries(
        Object.entries(components).map(([name, def]) => [
          name,
          { description: def.description, props: z.toJSONSchema(def.props) },
        ]),
      ),
    },
    null,
    2,
  ).replaceAll("\u2014", ",") + "\n";
const paths = [
  resolve(here, "../src/components/chat/ui/catalog.generated.json"),
  resolve(here, "../../backend/src/views/catalog.generated.json"),
];
for (const path of paths) {
  if (process.argv.includes("--check")) {
    if ((await readFile(path, "utf8")) !== content)
      throw new Error(`Regenerate catalog: ${path}`);
  } else await writeFile(path, content);
}
