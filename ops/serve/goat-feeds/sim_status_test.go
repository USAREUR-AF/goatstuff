// SPDX-License-Identifier: Apache-2.0

package main

import (
	"context"
	"encoding/json"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

func TestValidateSim(t *testing.T) {
	for _, m := range []string{"real", "simulated", "unknown", " Simulated "} {
		if _, _, err := ValidateSim(m, ""); err != nil {
			t.Errorf("mode %q refused: %v", m, err)
		}
	}
	for _, m := range []string{"", "sim", "fake", "REALISH"} {
		if _, _, err := ValidateSim(m, ""); err == nil || err.(*Refused).Status != 400 {
			t.Errorf("mode %q: %v, want a 400 refusal", m, err)
		}
	}
	mode, note, err := ValidateSim("simulated", "  ops desk says\n  autopilot is in   simulation  ")
	if err != nil || mode != "simulated" || note != "ops desk says autopilot is in simulation" {
		t.Errorf("cleaned: %q %q %v", mode, note, err)
	}
	if _, _, err := ValidateSim("real", strings.Repeat("é", SimNoteMax)); err != nil {
		t.Errorf("a %d-character note is refused: %v", SimNoteMax, err)
	}
	if _, _, err := ValidateSim("real", strings.Repeat("x", SimNoteMax+1)); err == nil {
		t.Error("a note over the limit is accepted")
	}
	if simEventDetail("real", "") != "real" || simEventDetail("simulated", "n") != "simulated: n" {
		t.Error("audit detail")
	}
}

// Who may declare: on a received feed, its requester, a tactical admin or a platform admin,
// while the feed is pending, on or paused. Never on a sending feed, never for another member.
func TestCanSimStatus(t *testing.T) {
	member := &Viewer{Email: "m@x", Role: RoleMember}
	owner := &Viewer{Email: "o@x", Role: RoleMember}
	tactical := &Viewer{Email: "t@x", Role: RoleTactical, IsAdmin: true}
	platform := &Viewer{Email: "p@x", Role: RolePlatform, IsAdmin: true}
	live := map[string]bool{"pending": true, "active": true, "paused": true}
	for _, dir := range []string{"in", "out", ""} {
		for _, st := range []string{"pending", "active", "paused", "declined", "expired", "removed"} {
			f := Feed{Owner: "O@X", Status: st, Direction: dir}
			want := dir == "in" && live[st]
			if Can(member, simActionName, f) {
				t.Errorf("%s/%s: another member may declare", dir, st)
			}
			for _, v := range []*Viewer{owner, tactical, platform} {
				if got := Can(v, simActionName, f); got != want {
					t.Errorf("%s/%s %s (%s): %v, want %v", dir, st, v.Email, v.Role, got, want)
				}
			}
		}
	}
	if got := ActionsFor(owner, Feed{Owner: "o@x", Status: "active", Direction: "in"}); strings.Join(got, ",") != "pause,remove,sim-status,check" {
		t.Errorf("owner's actions on a live received feed: %v", got)
	}
}

func TestSimStatusDoc(t *testing.T) {
	at := time.Unix(1764000000, 500_000_000)
	sim := &SimStatus{Mode: "simulated", SetAt: at, SetBy: "o@x", Note: "bench run"}
	feeds := []Feed{
		{ID: "feed-a", Direction: "in", Status: "active", Label: "drone-a position", Sim: sim},
		{ID: "feed-b", Direction: "in", Status: "active", Label: "not declared"},
		{ID: "feed-c", Direction: "in", Status: "removed", Label: "gone", Sim: sim},
		{ID: "feed-d", Direction: "in", Status: "declined", Label: "refused", Sim: sim},
		{ID: "feed-e", Direction: "out", Status: "active", Sim: sim},
		{ID: "feed-f", Direction: "in", Status: "paused", Label: "drone-b", Sim: &SimStatus{Mode: "real", SetAt: at, SetBy: "t@x"}},
	}
	d := simStatusDoc(feeds)
	b, _ := json.Marshal(d)
	var got map[string]any
	_ = json.Unmarshal(b, &got)
	if got["schema"] != "goat.sim-status/v1" {
		t.Errorf("schema: %v", got["schema"])
	}
	fs := got["feeds"].(map[string]any)
	if len(fs) != 2 || fs["feed-a"] == nil || fs["feed-f"] == nil {
		t.Fatalf("only declared, live received feeds: %v", fs)
	}
	a := fs["feed-a"].(map[string]any)
	if a["mode"] != "simulated" || a["set_by"] != "o@x" || a["note"] != "bench run" || a["label"] != "drone-a position" || a["set_at"] != 1764000000.5 {
		t.Errorf("feed-a: %v", a)
	}
	if len(a) != 5 {
		t.Errorf("feed-a has fields %v, want mode, set_at, set_by, note, label", a)
	}
	if empty := simStatusDoc(nil); empty.Feeds == nil {
		t.Error("an empty document has feeds: null, want {}")
	}
}

func simServer(t *testing.T) (*Server, *memReg) {
	t.Helper()
	c := mustConfig(t)
	reg := newMem()
	return &Server{cfg: c, reg: reg, sup: NewSupervisor(c, reg, (&fakeSpawner{}).spawn, time.Hour), peers: NewPeers("", ""),
		start: time.Now()}, reg
}

func TestSimStatusActionAndAPI(t *testing.T) {
	s, reg := simServer(t)
	ctx := context.Background()
	in, _ := reg.Create(ctx, Feed{Direction: "in", Status: "active", Owner: "member@example.test", Label: "drone-a position",
		OutputID: "mavlink2", Port: 7100, Transport: "udp", EndAt: time.Now().Add(time.Hour)}, "x", "")
	out, _ := reg.Create(ctx, Feed{Direction: "out", Status: "active", Owner: "member@example.test", TopicID: "in-" + in.ID,
		OutputID: "cot", EndAt: time.Now().Add(time.Hour)}, "x", "")

	// Refusals: another member, a sending feed, a bad mode, a long note.
	if code, b := call(t, s, "POST", "/api/feeds/"+in.ID+"/sim-status", "other@example.test", "", map[string]any{"mode": "real"}); code != 403 ||
		!strings.Contains(b["error"].(string), "real or simulated") {
		t.Errorf("another member: %d %v", code, b)
	}
	if code, _ := call(t, s, "POST", "/api/feeds/"+out.ID+"/sim-status", "member@example.test", "", map[string]any{"mode": "real"}); code != 403 {
		t.Errorf("on a sending feed: %d, want 403", code)
	}
	if code, _ := call(t, s, "POST", "/api/feeds/"+in.ID+"/sim-status", "member@example.test", "", map[string]any{"mode": "maybe"}); code != 400 {
		t.Errorf("bad mode: %d, want 400", code)
	}
	if code, _ := call(t, s, "POST", "/api/feeds/"+in.ID+"/sim-status", "member@example.test", "",
		map[string]any{"mode": "real", "note": strings.Repeat("x", 301)}); code != 400 {
		t.Errorf("long note: %d, want 400", code)
	}
	if got, _ := reg.Get(ctx, in.ID); got.Sim != nil {
		t.Fatalf("a refused declaration was stored: %+v", got.Sim)
	}

	// The requester declares; the feed's state is unchanged; the audit log names them.
	code, b := call(t, s, "POST", "/api/feeds/"+in.ID+"/sim-status", "member@example.test", "",
		map[string]any{"mode": "simulated", "note": "ops desk: <b>bench</b> run"})
	if code != 200 || b["status"] != "active" {
		t.Fatalf("declare: %d %v", code, b)
	}
	sm := b["sim_status"].(map[string]any)
	if sm["mode"] != "simulated" || sm["set_by"] != "member@example.test" || sm["note"] != "ops desk: <b>bench</b> run" {
		t.Errorf("sim_status: %v", sm)
	}
	last := reg.events[len(reg.events)-1]
	if last.Action != "sim-status" || last.Actor != "member@example.test" || last.Detail != "simulated: ops desk: <b>bench</b> run" {
		t.Errorf("audit: %+v", last)
	}
	// Admins of both kinds may change it.
	for _, g := range []string{"TEST-TACTICAL-ADMIN", "PLATFORM-ADMIN"} {
		if code, _ := call(t, s, "POST", "/api/feeds/"+in.ID+"/sim-status", "admin@example.test", g, map[string]any{"mode": "real"}); code != 200 {
			t.Errorf("%s: %d", g, code)
		}
	}
	_, _ = call(t, s, "POST", "/api/feeds/"+in.ID+"/sim-status", "admin@example.test", "PLATFORM-ADMIN", map[string]any{"mode": "simulated", "note": "bench run"})

	// GET /api/sim-status needs no sign-in and carries only declared feeds.
	r := httptest.NewRequest("GET", "/api/sim-status", nil)
	w := httptest.NewRecorder()
	s.Routes().ServeHTTP(w, r)
	if w.Code != 200 || w.Header().Get("Content-Type") != "application/json" {
		t.Fatalf("sim-status: %d %s", w.Code, w.Header().Get("Content-Type"))
	}
	var doc struct {
		Schema string                    `json:"schema"`
		Feeds  map[string]map[string]any `json:"feeds"`
	}
	if err := json.Unmarshal(w.Body.Bytes(), &doc); err != nil {
		t.Fatal(err)
	}
	if doc.Schema != SimSchema || len(doc.Feeds) != 1 || doc.Feeds[in.ID]["mode"] != "simulated" || doc.Feeds[in.ID]["label"] != "drone-a position" {
		t.Errorf("sim-status doc: %+v", doc)
	}
	if at, ok := doc.Feeds[in.ID]["set_at"].(float64); !ok || at < 1.7e9 {
		t.Errorf("set_at is Unix seconds: %v", doc.Feeds[in.ID]["set_at"])
	}
	if code, _ := call(t, s, "POST", "/api/sim-status", "admin@example.test", "PLATFORM-ADMIN", map[string]any{}); code != 405 {
		t.Errorf("POST /api/sim-status: %d, want 405 (read-only)", code)
	}

	// The sending feed's page shows its source's declaration.
	_, d := call(t, s, "GET", "/api/feeds/"+out.ID, "member@example.test", "", nil)
	if ss := d["feed"].(map[string]any)["source_sim_status"]; ss == nil || ss.(map[string]any)["mode"] != "simulated" {
		t.Errorf("source_sim_status on the sending feed: %v", ss)
	}
}

func TestCatalogCarriesTheDeclaration(t *testing.T) {
	s, reg := simServer(t)
	ctx := context.Background()
	in, _ := reg.Create(ctx, Feed{Direction: "in", Status: "active", Owner: "o@x", Label: "drone-a position", OutputID: "mavlink2",
		Port: 7100, Transport: "udp", EndAt: time.Now().Add(time.Hour)}, "x", "")
	if err := s.sup.Converge(ctx); err != nil {
		t.Fatal(err)
	}
	topicSim := func() any {
		_, cat := call(t, s, "GET", "/api/catalog", "o@x", "", nil)
		for _, tp := range cat["topics"].([]any) {
			if tp.(map[string]any)["id"] == "in-"+in.ID {
				return tp.(map[string]any)["sim_status"]
			}
		}
		t.Fatal("the received feed is not in the catalog")
		return nil
	}
	if got := topicSim(); got != nil {
		t.Errorf("before a declaration: %v", got)
	}
	_, _ = reg.SetSim(ctx, in.ID, "simulated", "o@x", "bench")
	if got := topicSim(); got == nil || got.(map[string]any)["mode"] != "simulated" {
		t.Errorf("after a declaration, at once (no converge): %v", got)
	}
	// Catalog topics that are not received feeds never carry one.
	_, cat := call(t, s, "GET", "/api/catalog", "o@x", "", nil)
	for _, tp := range cat["topics"].([]any) {
		m := tp.(map[string]any)
		if m["id"] == "fused" && m["sim_status"] != nil {
			t.Error("a catalog topic carries a declaration")
		}
	}
}
