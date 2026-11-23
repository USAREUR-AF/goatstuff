// SPDX-License-Identifier: Apache-2.0
package main

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

const testEstate = `{
 "regions": [{"region_id": "north", "label": "North"}, {"region_id": "south", "label": "South"}],
 "sites": [
  {"site_id": "alpha", "label": "Alpha", "region_id": "north", "systems": [
    {"name": "radar-1", "class": "radar"},
    {"name": "radar-2", "class": "radar", "duty_cycled": true},
    {"name": "node-1", "class": "node", "peer": "node-1.mesh.example"}]},
  {"site_id": "bravo", "label": "Bravo", "region_id": "south", "systems": []}
 ]}`

func testEstateFile(t *testing.T) *Estate {
	t.Helper()
	p := filepath.Join(t.TempDir(), "estate.json")
	if err := os.WriteFile(p, []byte(testEstate), 0o644); err != nil {
		t.Fatal(err)
	}
	e, err := LoadEstate(p, []SystemDecl{{Name: "console-b", Label: "Console B", Site: "bravo", Class: "console", Peer: "console-b"}})
	if err != nil {
		t.Fatal(err)
	}
	return e
}

func row(status string, integ, flow bool, dest string) feedOut {
	return feedOut{Feed: Feed{Status: status, Direction: "out", DestName: dest, Owner: "o@example.test"},
		Integrated: Signal{On: integ}, Flowing: Signal{On: flow}, Kind: "raw", OutputLabel: "CAT062"}
}

func TestFacetsJoinEstate(t *testing.T) {
	e := testEstateFile(t)
	o := row("active", true, true, "console-b")
	src := []SourceRef{{Label: "Radar 1", Site: "alpha", System: "radar-1"}}
	f := facetsFor(e, &o, src, &Peer{Name: "console-b", Groups: []string{"All", "NET-P2P", "NET-EFFECTOR-CUE"}}, "NET")
	if f.DestSite != "Bravo" || f.DestRegion != "South" || f.DestClass != "console" || f.DestRole != "EFFECTOR-CUE" || !f.DestKnown {
		t.Errorf("dest facets: %+v", f)
	}
	if len(f.SourceSites) != 1 || f.SourceSites[0] != "Alpha" || f.SourceRegions[0] != "North" || !f.SourcesKnown {
		t.Errorf("source facets: %+v", f)
	}
	if f.Attention {
		t.Errorf("healthy declared link flagged: %v", f.Reasons)
	}
}

func TestFacetsAttentionRules(t *testing.T) {
	e := testEstateFile(t)
	peer := &Peer{Name: "console-b"}
	duty := []SourceRef{{Site: "alpha", System: "radar-2"}}
	plain := []SourceRef{{Site: "alpha", System: "radar-1"}}
	cases := []struct {
		name      string
		o         feedOut
		src       []SourceRef
		peer      *Peer
		attention bool
		atRest    bool
	}{
		{"duty-cycled sensor idle reads at rest", row("active", true, false, "console-b"), duty, peer, false, true},
		{"plain sensor idle needs attention", row("active", true, false, "console-b"), plain, peer, true, false},
		{"on but not integrated", row("active", false, false, "console-b"), plain, peer, true, false},
		{"waiting for approval", row("pending", false, false, "console-b"), plain, peer, true, false},
		{"paused is a choice", row("paused", false, false, "console-b"), plain, peer, false, false},
		{"unknown destination", row("active", true, true, "stranger"), plain, &Peer{Name: "stranger"}, true, false},
		{"unknown source", row("active", true, true, "console-b"), []SourceRef{{Label: "?"}}, peer, true, false},
	}
	for _, c := range cases {
		f := facetsFor(e, &c.o, c.src, c.peer, "")
		if f.Attention != c.attention || f.AtRest != c.atRest {
			t.Errorf("%s: attention=%v atRest=%v reasons=%v", c.name, f.Attention, f.AtRest, f.Reasons)
		}
	}
	// No estate configured: estate reasons are off, so nothing is flagged for being undeclared.
	empty, _ := LoadEstate("", nil)
	o := row("active", true, true, "stranger")
	if f := facetsFor(empty, &o, nil, nil, ""); f.Attention {
		t.Errorf("no estate, still flagged: %v", f.Reasons)
	}
}

func TestFilterAndSummary(t *testing.T) {
	e := testEstateFile(t)
	mk := func(flow bool, src SourceRef) feedOut {
		o := row("active", true, flow, "console-b")
		o.Facets = facetsFor(e, &o, []SourceRef{src}, &Peer{Name: "console-b"}, "")
		return o
	}
	rows := []feedOut{mk(true, SourceRef{Site: "alpha", System: "radar-1"}), mk(false, SourceRef{Site: "alpha", System: "radar-1"}),
		mk(false, SourceRef{Site: "alpha", System: "radar-2"})}
	if got := filterRows(rows, map[string][]string{"state": {"attention"}}); len(got) != 1 {
		t.Errorf("attention filter: %d rows", len(got))
	}
	if got := filterRows(rows, map[string][]string{"source_site": {"alpha"}, "dest_site": {"BRAVO"}}); len(got) != 3 {
		t.Errorf("site filters: %d rows", len(got))
	}
	if got := filterRows(rows, map[string][]string{"q": {"radar-2"}}); len(got) != 1 {
		t.Errorf("text filter: %d rows", len(got))
	}
	sum := summarize(rows, "site")
	b, _ := json.Marshal(sum)
	if len(sum) != 2 || sum[0].Key != "Alpha" || sum[0].Total != 3 || sum[0].Sending != 1 || sum[0].Attention != 1 || sum[0].AtRest != 1 {
		t.Errorf("summary: %s", b)
	}
}
