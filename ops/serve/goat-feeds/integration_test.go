// SPDX-License-Identifier: Apache-2.0

package main

// Integration tests against a real Postgres. They run when GOAT_FEEDS_TEST_DSN names a database
// the tests may wipe (CI starts one as a service container); otherwise they skip.

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/jackc/pgx/v5/pgxpool"
)

func testDSN(t *testing.T) string {
	t.Helper()
	dsn := os.Getenv("GOAT_FEEDS_TEST_DSN")
	if dsn == "" {
		t.Skip("GOAT_FEEDS_TEST_DSN not set: Postgres integration tests skipped")
	}
	return dsn
}

// freshStore drops the tables and opens a store, so each test starts empty.
func freshStore(t *testing.T) *PGStore {
	t.Helper()
	dsn, ctx := testDSN(t), context.Background()
	pool, err := pgxpool.New(ctx, dsn)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := pool.Exec(ctx, `DROP TABLE IF EXISTS feeds, feed_events, placements`); err != nil {
		t.Fatal(err)
	}
	pool.Close()
	s, err := OpenPG(ctx, dsn)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { s.pool.Close() })
	return s
}

func TestPGMigratesTheFirstSchema(t *testing.T) {
	dsn, ctx := testDSN(t), context.Background()
	pool, err := pgxpool.New(ctx, dsn)
	if err != nil {
		t.Fatal(err)
	}
	defer pool.Close()
	// The schema the first build deployed: no direction, no label.
	if _, err := pool.Exec(ctx, `DROP TABLE IF EXISTS feeds, feed_events;
CREATE TABLE feeds (id TEXT PRIMARY KEY, topic_id TEXT NOT NULL, output_id TEXT NOT NULL, dest_name TEXT NOT NULL,
  dest_ip TEXT NOT NULL, dest_peer_id TEXT NOT NULL DEFAULT '', port INTEGER NOT NULL, transport TEXT NOT NULL,
  purpose TEXT NOT NULL DEFAULT '', owner TEXT NOT NULL, status TEXT NOT NULL, created_at TIMESTAMPTZ NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL, decided_by TEXT NOT NULL DEFAULT '', decided_at TIMESTAMPTZ,
  decision_note TEXT NOT NULL DEFAULT '', end_at TIMESTAMPTZ NOT NULL);
CREATE TABLE feed_events (id BIGSERIAL PRIMARY KEY, feed_id TEXT NOT NULL, at TIMESTAMPTZ NOT NULL, actor TEXT NOT NULL,
  action TEXT NOT NULL, detail TEXT NOT NULL DEFAULT '');
INSERT INTO feeds VALUES ('feed-old','fused','cat062','nav-1','198.51.100.1','',7005,'udp','air picture','o@x','active',
  now(),now(),'a@x',now(),'',now() + interval '10 days');`); err != nil {
		t.Fatal(err)
	}
	s, err := OpenPG(ctx, dsn)
	if err != nil {
		t.Fatalf("opening a first-build database: %v", err)
	}
	defer s.pool.Close()
	f, err := s.Get(ctx, "feed-old")
	if err != nil || f.Direction != "out" || f.Label != "" || f.Status != "active" || f.Sim != nil {
		t.Fatalf("the first build's row after migration: %+v %v", f, err)
	}
	// Opening again is a no-op.
	s2, err := OpenPG(ctx, dsn)
	if err != nil {
		t.Fatalf("second open: %v", err)
	}
	s2.pool.Close()
}

func TestPGLifecycle(t *testing.T) {
	s, ctx := freshStore(t), context.Background()
	if err := s.Ping(ctx); err != nil {
		t.Fatal(err)
	}
	now := time.Now()
	f, err := s.Create(ctx, Feed{TopicID: "fused", OutputID: "cat062", DestName: "nav-1", DestIP: "198.51.100.1", Port: 7005,
		Transport: "udp", Purpose: "p", Owner: "o@x", Status: "active", DecidedBy: "a@x", DecidedAt: &now,
		DecisionNote: "on", EndAt: now.Add(time.Hour), Direction: "out"}, "a@x", "detail")
	if err != nil || !strings.HasPrefix(f.ID, "feed-") || f.Direction != "out" {
		t.Fatalf("create: %+v %v", f, err)
	}
	ev, _ := s.Events(ctx, f.ID, 10)
	if len(ev) != 2 || ev[0].Action != "on" || ev[1].Action != "request" {
		t.Errorf("an active create records request then on: %+v", ev)
	}
	if _, err := s.Get(ctx, "feed-nope"); err != ErrNotFound {
		t.Errorf("missing feed: %v", err)
	}
	if _, err := s.Transition(ctx, "feed-nope", "paused", "a", "pause", "", false, nil); err != ErrNotFound {
		t.Errorf("transition of a missing feed: %v", err)
	}
	end := now.Add(48 * time.Hour)
	got, err := s.Transition(ctx, f.ID, "paused", "a@x", "pause", "maintenance", true, &end)
	if err != nil || got.Status != "paused" || got.DecidedBy != "a@x" || got.DecisionNote != "maintenance" || !got.EndAt.Round(time.Second).Equal(end.Round(time.Second)) {
		t.Errorf("transition: %+v %v", got, err)
	}
	list, _ := s.List(ctx, "paused")
	all, _ := s.List(ctx)
	if len(list) != 1 || len(all) != 1 {
		t.Errorf("list paused %d, all %d", len(list), len(all))
	}
	audit, _ := s.Events(ctx, "", 100)
	if len(audit) != 3 {
		t.Errorf("site audit: %d events, want 3", len(audit))
	}
	// Expiry moves only live feeds past their end date.
	past := now.Add(-time.Minute)
	g, _ := s.Create(ctx, Feed{TopicID: "fused", OutputID: "cat062", DestName: "n", DestIP: "198.51.100.2", Port: 7005,
		Transport: "udp", Owner: "o@x", Status: "active", EndAt: past, Direction: "out"}, "a", "")
	ids, err := s.ExpireDue(ctx, now)
	if err != nil || len(ids) != 1 || ids[0] != g.ID {
		t.Fatalf("expire: %v %v", ids, err)
	}
	g, _ = s.Get(ctx, g.ID)
	if g.Status != "expired" {
		t.Errorf("expired feed is %s", g.Status)
	}
}

func TestPGPortNamespaces(t *testing.T) {
	s, ctx := freshStore(t), context.Background()
	end := time.Now().Add(time.Hour)
	_, _ = s.Create(ctx, Feed{TopicID: "fused", OutputID: "cat062", DestName: "n", DestIP: "198.51.100.1", Port: 7100,
		Transport: "udp", Owner: "o", Status: "active", EndAt: end, Direction: "out"}, "a", "")
	in, _ := s.Create(ctx, Feed{TopicID: "intake", OutputID: "cot", DestName: "s", DestIP: "198.51.100.7", Port: 7101,
		Transport: "udp", Owner: "o", Status: "pending", EndAt: end, Direction: "in", Label: "Blue"}, "a", "")
	check := func(name string, got bool, err error, want bool) {
		if err != nil || got != want {
			t.Errorf("%s: %v %v, want %v", name, got, err, want)
		}
	}
	used, err := s.PortInUse(ctx, "198.51.100.1", 7100, "udp")
	check("out port in use", used, err, true)
	used, err = s.ListenPortInUse(ctx, 7100, "udp")
	check("an out feed does not hold a receive port", used, err, false)
	used, err = s.ListenPortInUse(ctx, 7101, "udp")
	check("pending receive feed holds its port", used, err, true)
	used, err = s.PortInUse(ctx, "198.51.100.7", 7101, "udp")
	check("a receive feed does not hold a destination port", used, err, false)
	_, _ = s.Transition(ctx, in.ID, "declined", "a", "decline", "no", true, nil)
	used, err = s.ListenPortInUse(ctx, 7101, "udp")
	check("a declined receive feed releases its port", used, err, false)
}

func TestPGConcurrentCreates(t *testing.T) {
	s, ctx := freshStore(t), context.Background()
	var wg sync.WaitGroup
	errs := make(chan error, 40)
	for i := 0; i < 40; i++ {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			_, err := s.Create(ctx, Feed{TopicID: "fused", OutputID: "cat062", DestName: "n", DestIP: "198.51.100.1",
				Port: 7000 + i, Transport: "udp", Owner: "o", Status: "pending", EndAt: time.Now().Add(time.Hour), Direction: "out"}, "a", "")
			errs <- err
		}(i)
	}
	wg.Wait()
	close(errs)
	for err := range errs {
		if err != nil {
			t.Fatal(err)
		}
	}
	all, _ := s.List(ctx)
	if len(all) != 40 {
		t.Errorf("40 concurrent creates: %d rows", len(all))
	}
}

// ---- HTTP over the real store ----

type apiClient struct {
	t    *testing.T
	base string
}

func (c apiClient) do(method, path, email, groups string, body any, withCSRF bool) (int, map[string]any) {
	c.t.Helper()
	var rd io.Reader
	if body != nil {
		b, _ := json.Marshal(body)
		rd = strings.NewReader(string(b))
	}
	req, _ := http.NewRequest(method, c.base+path, rd)
	if email != "" {
		req.Header.Set("X-Forwarded-Email", email)
		req.Header.Set("X-Forwarded-Groups", groups)
	}
	if withCSRF {
		req.Header.Set("X-Goat-Feeds", "1")
	}
	req.Header.Set("Content-Type", "application/json")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		c.t.Fatal(err)
	}
	defer resp.Body.Close()
	raw, _ := io.ReadAll(resp.Body)
	out := map[string]any{}
	_ = json.Unmarshal(raw, &out)
	if len(out) == 0 {
		out["_raw"] = string(raw)
	}
	return resp.StatusCode, out
}

const (
	memberEmail   = "member@example.test"
	tacticalEmail = "tactical@example.test"
)

func apiServer(t *testing.T) (apiClient, *Supervisor, *fakeSpawner) {
	t.Helper()
	s := freshStore(t)
	c := mustConfig(t)
	sp := &fakeSpawner{}
	sup := NewSupervisor(c, s, sp.spawn, time.Hour)
	srv := &Server{cfg: c, reg: s, sup: sup, peers: NewPeers("", ""), start: time.Now()}
	ts := httptest.NewServer(srv.Routes())
	t.Cleanup(func() { ts.Close(); sup.StopAll() })
	return apiClient{t: t, base: ts.URL}, sup, sp
}

func TestAPIAuthAndRefusals(t *testing.T) {
	api, _, _ := apiServer(t)
	if code, _ := api.do("GET", "/api/feeds", "", "", nil, false); code != 401 {
		t.Errorf("no sign-in: %d, want 401", code)
	}
	if code, _ := api.do("POST", "/api/feeds", memberEmail, "DEVELOPER", map[string]any{}, false); code != 403 {
		t.Errorf("POST without X-Goat-Feeds: %d, want 403", code)
	}
	if code, _ := api.do("POST", "/api/feeds", memberEmail, "DEVELOPER", nil, true); code != 400 {
		t.Errorf("empty body: %d, want 400", code)
	}
	code, me := api.do("GET", "/api/me", tacticalEmail, "TEST-TACTICAL-ADMIN", nil, false)
	if code != 200 || me["viewer"].(map[string]any)["role"] != RoleTactical {
		t.Errorf("me: %d %v", code, me)
	}
	if code, body := api.do("GET", "/api/feeds/feed-nope", memberEmail, "", nil, false); code != 404 {
		t.Errorf("missing feed: %d %v", code, body)
	}
	if code, _ := api.do("GET", "/healthz", "", "", nil, false); code != 200 {
		t.Errorf("healthz without sign-in: %d, want 200", code)
	}
}

func TestAPIOutLifecycle(t *testing.T) {
	api, sup, sp := apiServer(t)
	ctx := context.Background()
	req := map[string]any{"topic_id": "fused", "output_id": "cat062", "dest_name": "console-west", "dest_ip": "198.51.100.21",
		"port": 7005, "transport": "udp", "days": 7, "purpose": "air picture to the console"}
	code, f := api.do("POST", "/api/feeds", memberEmail, "DEVELOPER", req, true)
	if code != 201 || f["status"] != "pending" || f["health"] != "waiting" {
		t.Fatalf("member's request: %d %v", code, f)
	}
	id := f["id"].(string)
	if code, _ := api.do("POST", "/api/feeds/"+id+"/approve", memberEmail, "DEVELOPER", map[string]any{}, true); code != 403 {
		t.Errorf("member approving: %d, want 403", code)
	}
	if code, _ := api.do("POST", "/api/feeds/"+id+"/nope", tacticalEmail, "TEST-TACTICAL-ADMIN", map[string]any{}, true); code != 403 && code != 404 {
		t.Errorf("unknown action: %d", code)
	}
	if code, _ := api.do("POST", "/api/feeds", tacticalEmail, "TEST-TACTICAL-ADMIN", req, true); code != 409 {
		t.Errorf("second feed to the same port: %d, want 409", code)
	}
	if code, _ := api.do("POST", "/api/feeds/"+id+"/approve", tacticalEmail, "TEST-TACTICAL-ADMIN", map[string]any{"note": "ok"}, true); code != 200 {
		t.Fatalf("approve: %d", code)
	}
	if err := sup.Converge(ctx); err != nil {
		t.Fatal(err)
	}
	if len(sp.procs) != 1 {
		t.Fatalf("an approved feed: %d processes", len(sp.procs))
	}
	go func() { _, _ = io.WriteString(sp.procs[0].w, "goat-xf-pipe-zenoh: in=4 converted=4 failed=0 4s\n") }()
	var got map[string]any
	for i := 0; i < 100; i++ {
		_, d := api.do("GET", "/api/feeds/"+id, memberEmail, "", nil, false)
		got = d["feed"].(map[string]any)
		if got["health"] == "sending" {
			break
		}
		time.Sleep(20 * time.Millisecond)
	}
	if got["health"] != "sending" {
		t.Errorf("health after counters: %v (%v)", got["health"], got["health_note"])
	}
	_, metrics := api.do("GET", "/metrics", "", "", nil, false)
	for _, want := range []string{`goat_feeds_feeds{state="active"} 1`, "goat_feeds_groups_running 1", "goat_feeds_feed_in_total{feed=\"" + id} {
		if !strings.Contains(metrics["_raw"].(string), want) {
			t.Errorf("metrics lack %q", want)
		}
	}
	code, chk := api.do("POST", "/api/feeds/"+id+"/check", memberEmail, "", map[string]any{}, true)
	if code != 200 || chk["udp_note"] == nil {
		t.Errorf("check: %d %v", code, chk)
	}
	// Owner pauses and resumes; decline is refused on an active feed; admin renews; owner removes.
	for _, step := range []struct {
		who, groups, action string
		body                map[string]any
		want                int
	}{
		{memberEmail, "", "pause", map[string]any{}, 200},
		{memberEmail, "", "resume", map[string]any{}, 200},
		{tacticalEmail, "TEST-TACTICAL-ADMIN", "decline", map[string]any{"note": "x"}, 403},
		{memberEmail, "", "renew", map[string]any{"days": 10}, 403},
		{tacticalEmail, "TEST-TACTICAL-ADMIN", "renew", map[string]any{"days": 999}, 400},
		{tacticalEmail, "TEST-TACTICAL-ADMIN", "renew", map[string]any{"days": 10}, 200},
		{memberEmail, "", "remove", map[string]any{}, 200},
	} {
		if code, b := api.do("POST", "/api/feeds/"+id+"/"+step.action, step.who, step.groups, step.body, true); code != step.want {
			t.Errorf("%s by %s: %d %v, want %d", step.action, step.who, code, b, step.want)
		}
	}
	_ = sup.Converge(ctx)
	if s := sup.Summary(); s.Groups != 0 {
		t.Errorf("after remove: %d groups", s.Groups)
	}
	_, audit := api.do("GET", "/api/audit", memberEmail, "", nil, false)
	if n := len(audit["events"].([]any)); n != 6 {
		t.Errorf("audit events: %d, want 6 (request, approve, pause, resume, renew, remove)", n)
	}
}

func TestAPIDeclineNeedsNote(t *testing.T) {
	api, _, _ := apiServer(t)
	req := map[string]any{"topic_id": "fused", "output_id": "sapient", "dest_name": "c", "dest_ip": "198.51.100.22",
		"port": 9000, "purpose": "testing a receiver"}
	_, f := api.do("POST", "/api/feeds", memberEmail, "", req, true)
	id := f["id"].(string)
	if code, _ := api.do("POST", "/api/feeds/"+id+"/decline", tacticalEmail, "TEST-TACTICAL-ADMIN", map[string]any{}, true); code != 400 {
		t.Errorf("decline without a note: %d, want 400", code)
	}
	code, d := api.do("POST", "/api/feeds/"+id+"/decline", tacticalEmail, "TEST-TACTICAL-ADMIN", map[string]any{"note": "use the sim topic"}, true)
	if code != 200 || d["status"] != "declined" || d["decision_note"] != "use the sim topic" {
		t.Errorf("decline: %d %v", code, d)
	}
}

func TestAPIInLifecycleAndCatalog(t *testing.T) {
	api, sup, sp := apiServer(t)
	ctx := context.Background()
	code, intake := api.do("GET", "/api/intake", memberEmail, "", nil, false)
	if code != 200 || intake["enabled"] != true || len(intake["ports"].([]any)) != 2 {
		t.Fatalf("intake: %d %v", code, intake)
	}
	req := map[string]any{"direction": "in", "format_id": "mavlink2", "label": "Blue C-UAS drone position", "dest_name": "console-west",
		"dest_ip": "198.51.100.42", "port": 7100, "purpose": "interceptor position into the picture"}
	code, f := api.do("POST", "/api/feeds", tacticalEmail, "TEST-TACTICAL-ADMIN", req, true)
	if code != 201 || f["status"] != "active" || f["listen_at"] != "198.51.100.9:7100/udp" || f["published_topic_id"] != "in-"+f["id"].(string) {
		t.Fatalf("receive request: %d %v", code, f)
	}
	id := f["id"].(string)
	_, intake = api.do("GET", "/api/intake", memberEmail, "", nil, false)
	if p := intake["ports"].([]any)[0].(map[string]any); p["free"] != false {
		t.Errorf("port 7100 after a receive feed: %v", p)
	}
	if err := sup.Converge(ctx); err != nil {
		t.Fatal(err)
	}
	_, cat := api.do("GET", "/api/catalog", memberEmail, "", nil, false)
	found := false
	for _, tp := range cat["topics"].([]any) {
		if tp.(map[string]any)["id"] == "in-"+id {
			found = true
		}
	}
	if !found {
		t.Fatal("the received feed is not in the catalog")
	}
	// Someone requests the received feed out; it runs as an ordinary delivery.
	out := map[string]any{"topic_id": "in-" + id, "output_id": "cot", "dest_name": "tak-server", "dest_ip": "198.51.100.55",
		"port": 4242, "purpose": "drone position to TAK"}
	if code, d := api.do("POST", "/api/feeds", tacticalEmail, "TEST-TACTICAL-ADMIN", out, true); code != 201 || d["status"] != "active" {
		t.Fatalf("requesting the received feed out: %d %v", code, d)
	}
	_ = sup.Converge(ctx)
	if len(sp.procs) != 2 {
		t.Fatalf("receive + reader: %d processes", len(sp.procs))
	}
	go func() { _, _ = io.WriteString(sp.procs[0].w, "goat-xf-pipe-zenoh: in=9 converted=9 failed=0 9s\n") }()
	var got map[string]any
	for i := 0; i < 100; i++ {
		_, d := api.do("GET", "/api/feeds/"+id, memberEmail, "", nil, false)
		got = d["feed"].(map[string]any)
		if got["health"] == "receiving" {
			break
		}
		time.Sleep(20 * time.Millisecond)
	}
	if got["health"] != "receiving" {
		t.Errorf("receive health: %v", got["health"])
	}
	code, chk := api.do("POST", "/api/feeds/"+id+"/check", memberEmail, "", map[string]any{}, true)
	if code != 200 || chk["receive_note"] == nil || chk["udp_note"] != nil {
		t.Errorf("check on a receive feed: %d %v", code, chk)
	}
	if code, _ := api.do("POST", "/api/feeds/"+id+"/remove", tacticalEmail, "TEST-TACTICAL-ADMIN", map[string]any{}, true); code != 200 {
		t.Fatal("remove")
	}
	_ = sup.Converge(ctx)
	_, cat = api.do("GET", "/api/catalog", memberEmail, "", nil, false)
	for _, tp := range cat["topics"].([]any) {
		if tp.(map[string]any)["id"] == "in-"+id {
			t.Error("a removed received feed is still in the catalog")
		}
	}
}

func TestPGSimStatus(t *testing.T) {
	s, ctx := freshStore(t), context.Background()
	f, err := s.Create(ctx, Feed{TopicID: "intake", OutputID: "mavlink2", DestName: "drone-a", DestIP: "198.51.100.42", Port: 7100,
		Transport: "udp", Purpose: "p", Owner: "o@x", Status: "active", EndAt: time.Now().Add(time.Hour), Direction: "in",
		Label: "drone-a position"}, "o@x", "")
	if err != nil || f.Sim != nil {
		t.Fatalf("a new feed has no declaration: %+v %v", f.Sim, err)
	}
	before := f.UpdatedAt
	got, err := s.SetSim(ctx, f.ID, "simulated", "o@x", "bench run")
	if err != nil || got.Sim == nil || got.Sim.Mode != "simulated" || got.Sim.SetBy != "o@x" || got.Sim.Note != "bench run" ||
		time.Since(got.Sim.SetAt) > time.Minute || got.Status != "active" || !got.UpdatedAt.Equal(before) {
		t.Fatalf("set: %+v %+v %v", got, got.Sim, err)
	}
	if _, err := s.SetSim(ctx, "feed-nope", "real", "o@x", ""); err != ErrNotFound {
		t.Errorf("missing feed: %v", err)
	}
	got, _ = s.SetSim(ctx, f.ID, "real", "t@x", "")
	if got.Sim.Mode != "real" || got.Sim.Note != "" || got.Sim.SetBy != "t@x" {
		t.Errorf("second declaration replaces the first: %+v", got.Sim)
	}
	ev, _ := s.Events(ctx, f.ID, 10)
	if len(ev) != 4 || ev[0].Action != "sim-status" || ev[0].Detail != "real" || ev[1].Detail != "simulated: bench run" || ev[1].Actor != "o@x" {
		t.Errorf("audit: %+v", ev)
	}
	list, _ := s.List(ctx)
	if len(list) != 1 || list[0].Sim == nil || list[0].Sim.Mode != "real" {
		t.Errorf("list carries the declaration: %+v", list)
	}
}

func TestAPISimStatus(t *testing.T) {
	api, sup, _ := apiServer(t)
	ctx := context.Background()
	in := map[string]any{"direction": "in", "format_id": "mavlink2", "label": "drone-a position", "dest_name": "drone-a",
		"dest_ip": "198.51.100.42", "port": 7100, "purpose": "drone position into the picture"}
	_, f := api.do("POST", "/api/feeds", memberEmail, "DEVELOPER", in, true)
	id := f["id"].(string)
	if f["status"] != "pending" || f["sim_status"] != nil {
		t.Fatalf("member's receive request: %v", f)
	}
	acts := f["actions"].([]any)
	if !strings.Contains(fmt.Sprint(acts), "sim-status") {
		t.Errorf("the requester's actions lack sim-status: %v", acts)
	}
	if code, _ := api.do("POST", "/api/feeds/"+id+"/sim-status", "other@example.test", "DEVELOPER", map[string]any{"mode": "real"}, true); code != 403 {
		t.Errorf("another member: %d, want 403", code)
	}
	if code, _ := api.do("POST", "/api/feeds/"+id+"/sim-status", memberEmail, "DEVELOPER", map[string]any{"mode": "real"}, false); code != 403 {
		t.Errorf("without X-Goat-Feeds: %d, want 403", code)
	}
	// A pending feed can be declared by its requester; it stays pending.
	code, d := api.do("POST", "/api/feeds/"+id+"/sim-status", memberEmail, "DEVELOPER", map[string]any{"mode": "simulated", "note": "bench run"}, true)
	if code != 200 || d["status"] != "pending" || d["sim_status"].(map[string]any)["mode"] != "simulated" {
		t.Fatalf("declare: %d %v", code, d)
	}
	if code, _ := api.do("POST", "/api/feeds/"+id+"/approve", tacticalEmail, "TEST-TACTICAL-ADMIN", map[string]any{}, true); code != 200 {
		t.Fatal("approve")
	}
	_ = sup.Converge(ctx)
	// The catalog topic carries it, and the open document serves it with no sign-in.
	_, cat := api.do("GET", "/api/catalog", memberEmail, "", nil, false)
	var topicSim any
	for _, tp := range cat["topics"].([]any) {
		if tp.(map[string]any)["id"] == "in-"+id {
			topicSim = tp.(map[string]any)["sim_status"]
		}
	}
	if topicSim == nil || topicSim.(map[string]any)["set_by"] != memberEmail {
		t.Errorf("catalog topic: %v", topicSim)
	}
	code, doc := api.do("GET", "/api/sim-status", "", "", nil, false)
	if code != 200 || doc["schema"] != "goat.sim-status/v1" {
		t.Fatalf("sim-status: %d %v", code, doc)
	}
	e := doc["feeds"].(map[string]any)[id].(map[string]any)
	if e["mode"] != "simulated" || e["note"] != "bench run" || e["label"] != "drone-a position" || e["set_by"] != memberEmail {
		t.Errorf("sim-status entry: %v", e)
	}
	if at, ok := e["set_at"].(float64); !ok || time.Since(time.Unix(int64(at), 0)) > time.Minute {
		t.Errorf("set_at: %v", e["set_at"])
	}
	// A tactical admin changes it; the audit log records both with their emails.
	if code, _ := api.do("POST", "/api/feeds/"+id+"/sim-status", tacticalEmail, "TEST-TACTICAL-ADMIN", map[string]any{"mode": "real"}, true); code != 200 {
		t.Fatal("tactical admin declares")
	}
	_, audit := api.do("GET", "/api/audit", memberEmail, "", nil, false)
	n := 0
	for _, ev := range audit["events"].([]any) {
		m := ev.(map[string]any)
		if m["action"] == "sim-status" {
			n++
			if m["actor"] != memberEmail && m["actor"] != tacticalEmail {
				t.Errorf("audit actor: %v", m)
			}
		}
	}
	if n != 2 {
		t.Errorf("sim-status audit events: %d, want 2", n)
	}
	// Removed: no longer declarable, no longer served.
	_, _ = api.do("POST", "/api/feeds/"+id+"/remove", tacticalEmail, "TEST-TACTICAL-ADMIN", map[string]any{}, true)
	if code, _ := api.do("POST", "/api/feeds/"+id+"/sim-status", tacticalEmail, "TEST-TACTICAL-ADMIN", map[string]any{"mode": "real"}, true); code != 403 {
		t.Errorf("declare on a removed feed: %d, want 403", code)
	}
	_, doc = api.do("GET", "/api/sim-status", "", "", nil, false)
	if len(doc["feeds"].(map[string]any)) != 0 {
		t.Errorf("a removed feed is still served: %v", doc)
	}
}

func TestAPIHealthzDegraded(t *testing.T) {
	api, sup, sp := apiServer(t)
	ctx := context.Background()
	req := map[string]any{"topic_id": "fused", "output_id": "cat062", "dest_name": "c", "dest_ip": "198.51.100.23",
		"port": 7005, "purpose": "air picture"}
	_, _ = api.do("POST", "/api/feeds", tacticalEmail, "TEST-TACTICAL-ADMIN", req, true)
	_ = sup.Converge(ctx)
	sp.procs[0].Stop()
	for i := 0; i < 100; i++ {
		if sup.Summary().Running == 0 {
			break
		}
		time.Sleep(10 * time.Millisecond)
	}
	code, h := api.do("GET", "/healthz", "", "", nil, false)
	if code != 503 || h["status"] != "degraded" || !strings.Contains(fmt.Sprint(h["reasons"]), "not running") {
		t.Errorf("healthz with a stopped process: %d %v", code, h)
	}
}

// The placement overlay in Postgres: write, read back with its archive and far side,
// replace, delete, and every write audited.
func TestPGPlacements(t *testing.T) {
	s, ctx := freshStore(t), context.Background()
	p := Placement{Key: "console-north-ops-01", Site: "north", Class: "console", Role: "ops", Peer: "198.51.100.40", State: "placed",
		By: "t@example.test", Status: "archived", FarSide: &FarSide{Label: "Partner network", Direction: "out"},
		Archive: &ArchiveInfo{By: "t@example.test", Returns: "new-system", Rejoin: "rejoin-needed", Restore: []string{"feed-1", "declared:x"}}}
	if err := s.PutPlacement(ctx, p, "archive", "d"); err != nil {
		t.Fatal(err)
	}
	got, err := s.GetPlacement(ctx, p.Key)
	if err != nil {
		t.Fatal(err)
	}
	if got.Archive == nil || got.Archive.Returns != "new-system" || len(got.Archive.Restore) != 2 || got.FarSide.Label != "Partner network" || got.At.IsZero() {
		t.Errorf("round trip: %+v", got)
	}
	p.Status, p.Archive, p.FarSide = "active", nil, nil
	if err := s.PutPlacement(ctx, p, "restore", "d"); err != nil {
		t.Fatal(err)
	}
	all, err := s.ListPlacements(ctx)
	if err != nil || len(all) != 1 || all[0].Archive != nil || all[0].FarSide != nil || all[0].Status != "active" {
		t.Errorf("replace: %+v %v", all, err)
	}
	if err := s.DeletePlacement(ctx, p.Key, "t@example.test", "dismiss", "promoted"); err != nil {
		t.Fatal(err)
	}
	if _, err := s.GetPlacement(ctx, p.Key); !errors.Is(err, ErrNotFound) {
		t.Errorf("after delete: %v", err)
	}
	if err := s.DeletePlacement(ctx, p.Key, "t@example.test", "dismiss", ""); !errors.Is(err, ErrNotFound) {
		t.Errorf("deleting twice: %v", err)
	}
	ev, _ := s.Events(ctx, "system:"+p.Key, 10)
	if len(ev) != 3 {
		t.Errorf("audit events: %+v", ev)
	}
}
