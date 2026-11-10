// SPDX-License-Identifier: Apache-2.0
//
// Which catalog rows to show by default. A catalog is a discovery surface;
// on a per-identity slot pool most org rows are people who registered and
// declared nothing, so the page folds "nothing declared" rows behind a
// toggle. Pure, so the rule is unit-testable without a page.

/**
 * @typedef {Object} DiscoverableRow
 * @property {number | null} [declared_topics] Live declared topic rows, verbatim from the portal. `null`/`undefined`
 *   = the portal did not say (predates the field).
 */

/** True when the row belongs in the default view: it has declared topics,
 *  OR the portal did not say — unknown is shown, never folded, so an old
 *  portal can't make a whole catalog look empty. */
export function isDiscoverable(row) {
  return row.declared_topics == null || row.declared_topics > 0;
}

/** Split rows into the default view and the folded remainder. */
export function foldEmptyOrgs(
  rows,
) {
  const shown = [];
  const folded = [];
  for (const r of rows) (isDiscoverable(r) ? shown : folded).push(r);
  return { shown, folded };
}
