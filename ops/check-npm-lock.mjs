import { readFileSync } from "node:fs";

const lockPath = process.argv[2] || new URL("../web/package-lock.json", import.meta.url);
const lock = JSON.parse(readFileSync(lockPath, "utf8"));

const licenses = new Set(["0BSD", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "BlueOak-1.0.0", "CC0-1.0", "ISC", "MIT", "MIT-0", "MPL-2.0"]);
const registries = (process.env.APPROVED_NPM_REGISTRIES || "https://registry.npmjs.org/").split(",").map(value => {
  const url = new URL(value.trim());
  url.pathname = `${url.pathname.replace(/\/+$/, "")}/`;
  return url;
});
const errors = [];

const approvedSource = value => {
  try {
    const source = new URL(value);
    return source.protocol === "https:" && !source.username && !source.password && registries.some(registry =>
      source.origin === registry.origin && source.pathname.startsWith(registry.pathname)
    );
  } catch {
    return false;
  }
};

for (const [path, metadata] of Object.entries(lock.packages)) {
  if (!path) continue;
  if (!metadata.integrity) errors.push(`${path}: missing integrity`);
  if (!metadata.resolved) errors.push(`${path}: missing resolved source`);
  else if (!approvedSource(metadata.resolved)) errors.push(`${path}: unapproved source ${metadata.resolved}`);
  if (!licenses.has(metadata.license)) errors.push(`${path}: unapproved license ${metadata.license || "missing"}`);
}

if (errors.length) {
  console.error(errors.join("\n"));
  process.exit(1);
}
