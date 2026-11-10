// Bundles src/ into dist/ (index.html, app.js, app.css), which flowd embeds at cargo build time.
import { build as esbuild } from "esbuild";
import { copyFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));

/** Writes the bundle into `outdir`. */
export async function build(outdir = join(here, "dist")) {
  mkdirSync(outdir, { recursive: true });
  await esbuild({
    entryPoints: [join(here, "src/app.ts"), join(here, "src/app.css")],
    absWorkingDir: here,
    bundle: true,
    target: "es2022",
    outdir,
    logLevel: "warning",
  });
  copyFileSync(join(here, "src/index.html"), join(outdir, "index.html"));
}

if (process.argv[1] === fileURLToPath(import.meta.url)) await build();
