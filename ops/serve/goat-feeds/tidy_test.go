// SPDX-License-Identifier: Apache-2.0
package main

import (
	"bytes"
	"context"
	"encoding/json"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"
)

// memPlacements is an in-memory overlay store for tests.
type memPlacements struct {
	mu     sync.Mutex
	m      map[string]Placement
	events []Event
}

func newMemPlacements() *memPlacements { return &memPlacements{m: map[string]Placement{}} }

func (p *memPlacements) ListPlacements(context.Context) ([]Placement, error) {
	p.mu.Lock()
	defer p.mu.Unlock()
	out := []Placement{}
	for _, v := range p.m {
		out = append(out, v)
	}
	return out, nil
}
func (p *memPlacements) GetPlacement(_ context.Context, key string) (Placement, error) {
	p.mu.Lock()
	defer p.mu.Unlock()
	v, ok := p.m[key]
	if !ok {
		return v, ErrNotFound
	}
	return v, nil
}
func (p *memPlacements) PutPlacement(_ context.Context, pl Placement, action, detail string) error {
	p.mu.Lock()
	defer p.mu.Unlock()
	pl.At = time.Now()
	p.m[pl.Key] = pl
	p.events = append(p.events, Event{FeedID: "system:" + pl.Key, Actor: pl.By, Action: action, Detail: detail})
	return nil
}
func (p *memPlacements) DeletePlacement(_ context.Context, key, actor, action, detail string) error {
	p.mu.Lock()
	defer p.mu.Unlock()
	if _, ok := p.m[key]; !ok {
		return ErrNotFound
	}
	delete(p.m, key)
	p.events = append(p.events, Event{FeedID: "system:" + key, Actor: actor, Action: action, Detail: detail})
	return nil
}

const tidyEstate = `{
 "regions": [{"region_id": "north", "label": "North"}],
 "sites": [
  {"site_id": "north", "label": "North base", "region_id": "north", "lat": 1.5, "systems": [
    {"name": "console-north-ops-01", "class": "console", "role": "ops", "peer": "198.51.100.40"},
    {"name": "old-console", "class": "console", "peer": "198.51.100.41", "device_type": "kept"},
    {"name": "boundary-north-relay-01", "class": "boundary", "peer": "198.51.100.42"},
    {"name": "boundary-north-relay-02", "class": "boundary", "peer": "198.51.100.43", "far_side": {"label": "Partner network", "direction": "out"}},
    {"name": "radar-north-watch-01", "class": "radar", "peer": "198.51.100.44"}]},
  {"site_id": "harbor", "label": "Harbor", "region_id": "north", "systems": []}
 ]}`

func tidyServer(t *testing.T) (*Server, *memReg, *memPlacements) {
	t.Helper()
	p := filepath.Join(t.TempDir(), "estate.json")
	if err := os.WriteFile(p, []byte(tidyEstate), 0o644); err != nil {
		t.Fatal(err)
	}
	est, err := LoadEstate(p, nil)
	if err != nil {
		t.Fatal(err)
	}
	c := mustConfig(t)
	reg, pl := newMem(), newMemPlacements()
	s := &Server{cfg: c, reg: reg, sup: NewSupervisor(c, reg, (&fakeSpawner{}).spawn, time.Hour), peers: NewPeers("", ""),
		est: est, pl: pl, start: time.Now()}
	if err := s.reloadOverlay(context.Background()); err != nil {
		t.Fatal(err)
	}
	return s, reg, pl
}

const (
	tacticalH = "TEST-TACTICAL-ADMIN"
)

func call(t *testing.T, s *Server, method, path, email, groups string, body any) (int, map[string]any) {
	t.Helper()
	var rd *bytes.Reader
	if body != nil {
		b, _ := json.Marshal(body)
		rd = bytes.NewReader(b)
	} else {
		rd = bytes.NewReader(nil)
	}
	r := httptest.NewRequest(method, path, rd)
	r.Header.Set("X-Forwarded-Email", email)
	r.Header.Set("X-Forwarded-Groups", groups)
	r.Header.Set("X-Goat-Feeds", "1")
	w := httptest.NewRecorder()
	s.Routes().ServeHTTP(w, r)
	out := map[string]any{}
	_ = json.Unmarshal(w.Body.Bytes(), &out)
	return w.Code, out
}

func TestBoundaryAndArchiveFacets(t *testing.T) {
	s, _, _ := tidyServer(t)
	e := s.estate()
	o := row("active", true, true, "x")
	o.DestIP = "198.51.100.43"
	f := facetsFor(e, &o, nil, nil, "")
	if f.Boundary != "To Partner network through boundary node boundary-north-relay-02" {
		t.Errorf("boundary phrase: %q", f.Boundary)
	}
	o.DestIP = "198.51.100.42"
	f = facetsFor(e, &o, nil, nil, "")
	if !strings.Contains(f.Boundary, "not declared") || !inList(f.Reasons, reasonBoundaryNoFarSide) {
		t.Errorf("boundary without far side: %q %v", f.Boundary, f.Reasons)
	}
	in := row("active", true, true, "x")
	in.Direction, in.DestIP = "in", "198.51.100.43"
	if f := facetsFor(e, &in, nil, nil, ""); !strings.HasPrefix(f.Boundary, "From Partner network") {
		t.Errorf("in boundary phrase: %q", f.Boundary)
	}
	// An archived destination: no attention even when nothing flows; state "archived".
	eff := s.est.WithOverlay([]Placement{{Key: "console-north-ops-01", Status: "archived", State: "placed", By: "a@example.test",
		Archive: &ArchiveInfo{Returns: "new-system", Rejoin: "rejoin-needed", Operation: "exercise one", At: "2026-01-02T00:00:00Z", By: "a@example.test"}}})
	o = row("active", false, false, "x")
	o.DestIP = "198.51.100.40"
	o.Facets = facetsFor(eff, &o, nil, nil, "")
	if !o.Facets.Archived || o.Facets.Attention || stateWord(o) != "archived" {
		t.Errorf("archived facets: %+v", o.Facets)
	}
	if !strings.Contains(o.Facets.ArchiveNote, "replaced by a new system") || !strings.Contains(o.Facets.ArchiveNote, "rejoin") ||
		!strings.Contains(o.Facets.ArchiveNote, "exercise one") {
		t.Errorf("archive note: %q", o.Facets.ArchiveNote)
	}
	g := summarize([]feedOut{o}, "state")
	if len(g) != 1 || g[0].Archived != 1 || g[0].Off != 0 || g[0].Attention != 0 {
		t.Errorf("summary: %+v", g)
	}
}

func TestOverlayAndEffectiveJSON(t *testing.T) {
	s, _, _ := tidyServer(t)
	ps := []Placement{
		{Key: "old-console", Name: "console-harbor-ops-01", Site: "harbor", Role: "ops", State: "placed", By: "a@example.test"},
		{Key: "console-harbor-cue-01", Site: "harbor", Class: "console", Role: "cue", Peer: "198.51.100.50", State: "placed", By: "a@example.test"},
		{Key: "console-harbor-cue-02", Site: "harbor", Class: "console", Peer: "198.51.100.51", State: "proposed", By: "m@example.test"},
		{Key: "boundary-north-relay-01", FarSide: &FarSide{Label: "Allied network", Direction: "out"}, State: "placed", By: "a@example.test"},
	}
	eff := s.est.WithOverlay(ps)
	if d, ok := eff.ByName("console-harbor-ops-01"); !ok || d.Site != "harbor" || d.PlacedBy != "a@example.test" {
		t.Errorf("renamed and moved: %+v %v", d, ok)
	}
	if _, ok := eff.ByName("old-console"); ok {
		t.Errorf("the old name must be gone")
	}
	if d, ok := eff.ByPeer("198.51.100.50"); !ok || d.Name != "console-harbor-cue-01" {
		t.Errorf("new system by peer: %+v", d)
	}
	if _, ok := eff.ByName("console-harbor-cue-02"); ok {
		t.Errorf("a proposal must take no effect")
	}
	b, err := s.est.EffectiveJSON(ps)
	if err != nil {
		t.Fatal(err)
	}
	var doc estateFile
	if err := json.Unmarshal(b, &doc); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(b), `"device_type": "kept"`) || !strings.Contains(string(b), `"lat": 1.5`) {
		t.Errorf("fields this service does not read must be kept:\n%s", b)
	}
	harbor := map[string]bool{}
	for _, st := range doc.Sites {
		for _, sy := range st.Systems {
			if st.ID == "harbor" {
				harbor[sy.Name] = true
			}
			if sy.Name == "boundary-north-relay-01" && (sy.FarSide == nil || sy.FarSide.Label != "Allied network") {
				t.Errorf("far side not exported: %+v", sy)
			}
		}
	}
	if !harbor["console-harbor-ops-01"] || !harbor["console-harbor-cue-01"] || harbor["console-harbor-cue-02"] {
		t.Errorf("harbor systems in the export: %v", harbor)
	}
}

func TestTidyListsAndActs(t *testing.T) {
	s, reg, pl := tidyServer(t)
	ctx := context.Background()
	now := time.Now()
	// A feed to an undeclared system, one to an archived-to-be console, one to a boundary node without a far side.
	mk := func(ip string, port int) Feed {
		f, _ := reg.Create(ctx, Feed{TopicID: "fused", OutputID: "cat062", DestName: "dest", DestIP: ip, Port: port, Transport: "udp",
			Owner: "member@example.test", Status: "active", Direction: "out", EndAt: now.Add(time.Hour)}, "x", "")
		return f
	}
	undeclared := mk("198.51.100.60", 7001)
	toConsole := mk("198.51.100.40", 7002)
	mk("198.51.100.42", 7003)

	code, _ := call(t, s, "GET", "/api/tidy", "member@example.test", "DEVELOPER", nil)
	if code != 403 {
		t.Errorf("a member must not see Tidy up: %d", code)
	}
	code, body := call(t, s, "GET", "/api/tidy", "t@example.test", tacticalH, nil)
	if code != 200 {
		t.Fatalf("tidy: %d %v", code, body)
	}
	kinds := map[string]int{}
	for _, it := range body["items"].([]any) {
		kinds[it.(map[string]any)["kind"].(string)]++
	}
	if kinds["undeclared"] != 1 || kinds["boundary-far-side"] != 1 || kinds["name-rule"] < 1 {
		t.Errorf("tidy kinds: %v", kinds)
	}

	// Place the undeclared system: a bad name is refused with how to fix it.
	code, body = call(t, s, "POST", "/api/systems/Console60/place", "t@example.test", tacticalH,
		map[string]any{"site": "north", "class": "console", "role": "ops", "peer": "198.51.100.60"})
	if code != 400 || !strings.Contains(body["error"].(string), "naming rule") || !strings.Contains(body["error"].(string), "Suggested") {
		t.Errorf("bad name: %d %v", code, body)
	}
	code, body = call(t, s, "POST", "/api/systems/console-north-ops-02/place", "t@example.test", tacticalH,
		map[string]any{"site": "north", "class": "console", "role": "ops", "peer": "198.51.100.60", "label": "Ops console 2"})
	if code != 200 {
		t.Fatalf("place: %d %v", code, body)
	}
	if d, ok := s.estate().ByPeer("198.51.100.60"); !ok || d.PlacedBy != "t@example.test" {
		t.Errorf("placed system not effective: %+v", d)
	}
	o := s.decorate(undeclared, &Viewer{Email: "t@example.test", IsAdmin: true}, false)
	if !o.Facets.DestKnown || o.Facets.DestPlacedBy != "t@example.test" {
		t.Errorf("facets after place: %+v", o.Facets)
	}

	// Archive the console with pause: its feed pauses and is on the restore list.
	code, body = call(t, s, "POST", "/api/systems/console-north-ops-01/archive", "t@example.test", tacticalH,
		map[string]any{"returns": "same-system-same-site", "rejoin": "peer-kept", "operation": "exercise one", "pause": true})
	if code != 200 {
		t.Fatalf("archive: %d %v", code, body)
	}
	if f, _ := reg.Get(ctx, toConsole.ID); f.Status != "paused" {
		t.Errorf("archive with pause left the feed %s", f.Status)
	}
	d, _ := s.estate().ByName("console-north-ops-01")
	if !d.Archived() || d.Archive == nil || len(d.Archive.Restore) != 1 || d.Archive.Restore[0] != toConsole.ID {
		t.Errorf("archive record: %+v", d.Archive)
	}
	code, _ = call(t, s, "POST", "/api/systems/console-north-ops-01/archive", "t@example.test", tacticalH, map[string]any{"returns": "someday"})
	if code != 400 {
		t.Errorf("a bad returns value must be refused: %d", code)
	}

	// Bring it back: unarchived and the paused feed resumed.
	code, body = call(t, s, "POST", "/api/systems/console-north-ops-01/restore", "t@example.test", tacticalH, map[string]any{})
	if code != 200 {
		t.Fatalf("restore: %d %v", code, body)
	}
	if f, _ := reg.Get(ctx, toConsole.ID); f.Status != "active" {
		t.Errorf("restore left the feed %s", f.Status)
	}
	if d, _ := s.estate().ByName("console-north-ops-01"); d.Archived() {
		t.Errorf("still archived after restore")
	}
	code, _ = call(t, s, "POST", "/api/systems/console-north-ops-01/restore", "t@example.test", tacticalH, map[string]any{})
	if code != 409 {
		t.Errorf("restoring an active system: %d", code)
	}

	// Boundary far side through Place.
	code, body = call(t, s, "POST", "/api/systems/boundary-north-relay-01/place", "t@example.test", tacticalH,
		map[string]any{"far_side": map[string]string{"label": "Allied network", "direction": "out"}})
	if code != 200 {
		t.Fatalf("far side: %d %v", code, body)
	}
	// Every write is audited.
	actions := []string{}
	for _, e := range pl.events {
		actions = append(actions, e.Action)
	}
	if strings.Join(actions, ",") != "place,archive,restore,place" {
		t.Errorf("audit: %v", actions)
	}
	if code, _ := call(t, s, "POST", "/api/systems/x/archive", "member@example.test", "DEVELOPER", map[string]any{}); code != 403 {
		t.Errorf("a member must not archive: %d", code)
	}
}

func TestDeclareRuleOnRequests(t *testing.T) {
	s, _, pl := tidyServer(t)
	good := map[string]any{"direction": "out", "topic_id": "sim", "output_id": "cat062", "dest_name": "new box", "dest_ip": "198.51.100.70",
		"port": 7010, "transport": "udp", "days": 5, "purpose": "test feed"}
	code, body := call(t, s, "POST", "/api/feeds", "member@example.test", "DEVELOPER", good)
	if code != 400 || !strings.Contains(body["error"].(string), "not a declared system") {
		t.Fatalf("undeclared destination: %d %v", code, body)
	}
	good["declare"] = map[string]any{"name": "console-nowhere-ops-01", "site": "north", "class": "console", "role": "ops"}
	code, body = call(t, s, "POST", "/api/feeds", "member@example.test", "DEVELOPER", good)
	if code != 400 || !strings.Contains(body["error"].(string), "declared site id") {
		t.Fatalf("bad declare name: %d %v", code, body)
	}
	good["declare"] = map[string]any{"name": "console-north-ops-07", "site": "north", "class": "console", "role": "ops"}
	code, body = call(t, s, "POST", "/api/feeds", "member@example.test", "DEVELOPER", good)
	if code != 201 {
		t.Fatalf("member declare: %d %v", code, body)
	}
	if p := pl.m["console-north-ops-07"]; p.State != "proposed" || p.Peer != "198.51.100.70" || !strings.Contains(p.Reason, body["id"].(string)) {
		t.Errorf("member's declaration must be a proposal: %+v", p)
	}
	if _, ok := s.estate().ByName("console-north-ops-07"); ok {
		t.Errorf("a proposal must not take effect")
	}
	// The proposal shows in Tidy up; an admin places it.
	_, tb := call(t, s, "GET", "/api/tidy", "t@example.test", tacticalH, nil)
	found := false
	for _, it := range tb["items"].([]any) {
		found = found || it.(map[string]any)["kind"] == "proposed"
	}
	if !found {
		t.Errorf("proposal not in Tidy up: %v", tb["items"])
	}
	code, body = call(t, s, "POST", "/api/systems/console-north-ops-07/place", "t@example.test", tacticalH,
		map[string]any{"site": "north", "class": "console", "role": "ops", "peer": "198.51.100.70"})
	if code != 200 {
		t.Fatalf("place proposal: %d %v", code, body)
	}
	if p := pl.m["console-north-ops-07"]; p.State != "placed" {
		t.Errorf("proposal not placed: %+v", p)
	}
	// An admin's declaration is placed at once; a boundary node needs its far side.
	good["dest_ip"], good["port"] = "198.51.100.71", 7011
	good["declare"] = map[string]any{"name": "boundary-harbor-relay-01", "site": "harbor", "class": "boundary", "role": "relay"}
	code, body = call(t, s, "POST", "/api/feeds", "t@example.test", tacticalH, good)
	if code != 400 || !strings.Contains(body["error"].(string), "far side") {
		t.Fatalf("boundary without far side: %d %v", code, body)
	}
	good["declare"] = map[string]any{"name": "boundary-harbor-relay-01", "site": "harbor", "class": "boundary", "role": "relay",
		"far_side": map[string]string{"label": "Partner network", "direction": "out"}}
	code, body = call(t, s, "POST", "/api/feeds", "t@example.test", tacticalH, good)
	if code != 201 {
		t.Fatalf("admin declare: %d %v", code, body)
	}
	fc := body["facets"].(map[string]any)
	if fc["boundary"] != "To Partner network through boundary node boundary-harbor-relay-01" {
		t.Errorf("boundary on the new feed: %v", fc["boundary"])
	}
	// The naming API.
	code, nb := call(t, s, "GET", "/api/naming?name=console-north-ops-07&site=north&kind=console&function=ops", "member@example.test", "DEVELOPER", nil)
	if code != 200 || nb["suggestion"] != "console-north-ops-01" && nb["suggestion"] != "console-north-ops-02" {
		t.Errorf("naming: %d %v", code, nb["suggestion"])
	}
	if nb["taken"] != true || nb["check"].(map[string]any)["ok"] != true {
		t.Errorf("naming check: %v %v", nb["taken"], nb["check"])
	}
	// The effective estate is for admins.
	if code, _ := call(t, s, "GET", "/api/estate/effective", "member@example.test", "DEVELOPER", nil); code != 403 {
		t.Errorf("effective estate for a member: %d", code)
	}
}
