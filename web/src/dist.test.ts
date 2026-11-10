// web/dist is committed so `cargo build` needs no Node; this fails when it is stale.
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { build } from "../build.mjs";

test("web/dist matches a fresh build of web/src (run `npm --prefix web run build`)", async () => {
  const fresh = mkdtempSync(join(tmpdir(), "flow-web-"));
  await build(fresh);
  const dist = new URL("../dist/", import.meta.url);
  for (const file of ["index.html", "app.js", "app.css"]) {
    assert.equal(
      readFileSync(new URL(file, dist), "utf8"),
      readFileSync(join(fresh, file), "utf8"),
      `web/dist/${file} is stale`,
    );
  }
});
