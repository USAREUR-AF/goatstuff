// SPDX-License-Identifier: Apache-2.0
//
// The viewer chip: who the map thinks you are and how much you may see, from
// /api/whoami. Access rides the sign-in session, which lasts hours, so a person
// granted access after signing in keeps the old view until they sign out. When
// the map shows a signed-in viewer no tracks, it says why and how to fix it.
(function () {
  const VIEW = { full: 'all tracks', scoped: 'some sources', floor: 'no track access' };
  async function load() {
    let w;
    try { w = await (await fetch('api/whoami')).json(); } catch (e) { return; }
    window.viewer = w || {};
    if (!w || !w.enforced) return;            // no per-viewer access on this goatnet
    const box = document.getElementById('viewer');
    document.getElementById('viewer-who').textContent = w.email || 'signed in';
    const scope = w.view === 'scoped' ? `${w.grants} source${w.grants === 1 ? '' : 's'}` : VIEW[w.view] || w.view;
    document.getElementById('viewer-view').textContent = scope;
    const so = document.getElementById('viewer-signout');
    if (w.sign_out) { so.href = w.sign_out; so.hidden = false; }
    box.hidden = false;
    if (w.view === 'floor') {
      const n = document.getElementById('access-note');
      n.innerHTML = '';
      n.append('Your sign-in carries no track access, so no tracks are shown. If access was granted after you signed in, ');
      if (w.sign_out) {
        const a = document.createElement('a'); a.href = w.sign_out; a.textContent = 'sign out';
        n.append(a, ' and sign back in to pick it up.');
      } else {
        n.append('sign out and sign back in to pick it up.');
      }
      n.hidden = false;
    }
  }
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', load); else load();
})();
