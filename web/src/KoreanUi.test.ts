import { expect, test } from "vitest";

test("주요 섹션 제목은 한국어를 포함한다", () => {
  const untranslated: string[] = [];
  const sources = import.meta.glob("./*.tsx", {
    eager: true,
    import: "default",
    query: "?raw",
  }) as Record<string, string>;

  for (const [name, source] of Object.entries(sources)) {
    for (const match of source.matchAll(/className="eyebrow">([\s\S]*?)<\/p>/g)) {
      if (!/[가-힣]/.test(match[1])) untranslated.push(name);
    }
  }

  expect(untranslated).toEqual([]);
});
