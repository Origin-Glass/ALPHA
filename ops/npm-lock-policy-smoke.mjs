import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";

const directory = mkdtempSync(join(tmpdir(), "alpha-npm-policy-"));
const checker = new URL("./check-npm-lock.mjs", import.meta.url);
const metadata = resolved => ({
  packages: {
    "": { name: "fixture", version: "1.0.0" },
    "node_modules/fixture": { version: "1.0.0", resolved, integrity: "sha512-fixture", license: "MIT" }
  }
});

const check = (name, resolved, expected, registry) => {
  const path = join(directory, `${name}.json`);
  writeFileSync(path, `${JSON.stringify(metadata(resolved))}\n`);
  const env = { ...process.env, ...(registry ? { APPROVED_NPM_REGISTRIES: registry } : {}) };
  const result = spawnSync(process.execPath, [checker.pathname, path], { encoding: "utf8", env });
  if (result.status !== expected) throw new Error(`${name}: exit ${result.status}, expected ${expected}\n${result.stderr}`);
};

check("approved", "https://registry.npmjs.org/fixture/-/fixture-1.0.0.tgz", 0);
check("lookalike", "https://registry.npmjs.org.evil.example/fixture.tgz", 1);
check("prefix-lookalike", "https://packages.example/npm-evil/fixture.tgz", 1, "https://packages.example/npm/");
check("git", "git+https://github.com/example/fixture.git", 1);
check("file", "file:../fixture", 1);
console.log("npm lock policy smoke passed");
