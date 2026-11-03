// SPDX-License-Identifier: Apache-2.0
//
// Unit spec for the catalog default-view rule (src/catalog-filter.ts).
// Contract: rows with declared topics show; rows the portal positively
// reports as empty fold; rows the portal said nothing about show (an old
// portal must never make the catalog look empty).

import { test, expect } from "@playwright/test";
import { isDiscoverable, foldEmptyOrgs } from "../src/catalog-filter.js";

test("declared topics show, positively-empty folds, unknown shows", () => {
  expect(isDiscoverable({ declared_topics: 3 })).toBe(true);
  expect(isDiscoverable({ declared_topics: 0 })).toBe(false);
  expect(isDiscoverable({ declared_topics: null })).toBe(true);
  expect(isDiscoverable({})).toBe(true);
});

test("fold keeps order and loses nothing", () => {
  const rows = [
    { prefix: "a", declared_topics: 2 },
    { prefix: "b", declared_topics: 0 },
    { prefix: "c" },
    { prefix: "d", declared_topics: 0 },
  ];
  const { shown, folded } = foldEmptyOrgs(rows);
  expect(shown.map((r) => r.prefix)).toEqual(["a", "c"]);
  expect(folded.map((r) => r.prefix)).toEqual(["b", "d"]);
  expect(shown.length + folded.length).toBe(rows.length);
});
