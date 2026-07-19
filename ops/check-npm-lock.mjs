import lock from "../web/package-lock.json" with { type: "json" };

const licenses = new Set(["0BSD", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "BlueOak-1.0.0", "CC0-1.0", "ISC", "MIT", "MIT-0", "MPL-2.0"]);
const registries = (process.env.APPROVED_NPM_REGISTRIES || "https://registry.npmjs.org/").split(",").map(value => value.trim());
const errors = [];

for (const [path, metadata] of Object.entries(lock.packages)) {
  if (!path) continue;
  if (!metadata.integrity) errors.push(`${path}: missing integrity`);
  if (!metadata.resolved) errors.push(`${path}: missing resolved source`);
  else if (!registries.some(registry => metadata.resolved.startsWith(registry))) errors.push(`${path}: unapproved source ${metadata.resolved}`);
  if (!licenses.has(metadata.license)) errors.push(`${path}: unapproved license ${metadata.license || "missing"}`);
}

if (errors.length) {
  console.error(errors.join("\n"));
  process.exit(1);
}
