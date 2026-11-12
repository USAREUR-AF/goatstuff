// SPDX-License-Identifier: Apache-2.0
//
// Display-only lexicon for JS-generated labels. The static HTML gets its
// `__LEX_*__` tokens substituted at serve time (see src/embed.rs); the
// strings the frontend builds at runtime (side-panel rows, fold buttons,
// the super-node badge) read their wording from here instead.
//
// `LEX` starts at the default wording so a pre-bootstrap render (before the
// first /api/legend response) is byte-stable with what shipped before the
// lexicon existed. `applyLexicon()` updates it in place from the legend
// payload; the page title + masthead subtitle are re-applied there too,
// in case the static substitution was bypassed (dev server / stale dist).
//
// Render-time only: nothing here ever touches a key, ACL subject, cert
// field, or any machine path — it only changes the word shown. Mirror of
// the portal lexicon (docs/design/portal-lexicon.md §"Panoscope").


/**
 * @typedef {Object} Lexicon
 * @property {string} org participant noun, lower singular ("vendor" / "org")
 * @property {string} orgs participant noun, lower plural ("vendors" / "orgs")
 * @property {string} orgUpper participant noun, upper singular ("VENDOR" / "ORG")
 * @property {string} orgsUpper participant noun, upper plural ("VENDORS" / "ORGS")
 * @property {string} fabricLabel fabric label for the title + subtitle (for example "<site> fabric")
 */

/** Mutable singleton — default wording until the first legend response. */
export const LEX = {
  org: "vendor",
  orgs: "vendors",
  orgUpper: "VENDOR",
  orgsUpper: "VENDORS",
  fabricLabel: "Zenoh fabric",
};

/** Update `LEX` in place from a legend payload + re-apply title/subtitle.
 *  Tolerant of an older backend that omits `lexicon` (keeps defaults). */
export function applyLexicon(legend) {
  const l = legend.lexicon;
  if (!l) return;
  if (l.org) LEX.org = l.org;
  if (l.orgs) LEX.orgs = l.orgs;
  if (l.org_upper) LEX.orgUpper = l.org_upper;
  if (l.orgs_upper) LEX.orgsUpper = l.orgs_upper;
  if (l.fabric_label) LEX.fabricLabel = l.fabric_label;

  // Belt-and-suspenders: keep the document title + masthead subtitle in
  // sync even if serve-time HTML substitution didn't run (e.g. the Vite
  // dev server, or a dist built before the tokens landed). No-op when the
  // tokens were already resolved server-side.
  document.title = `panoscope — ${LEX.fabricLabel}`;
  const subtitle = document.querySelector("header .subtitle");
  if (subtitle) {
    subtitle.textContent = `Data Common Operating Picture · ${LEX.fabricLabel}`;
  }
}
