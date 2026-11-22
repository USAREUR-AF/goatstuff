// SPDX-License-Identifier: Apache-2.0

package main

import (
	"context"
	"io"
	"net/http"
	"strings"
	"sync"
	"testing"
	"time"
)

const testConfig = `{
  "site": "test-site", "tactical_groups": ["TEST-TACTICAL-ADMIN"],
  "catalog": [
    {"id": "fused", "label": "Fused picture", "key": "slot/fused/tracks", "kind": "processed",
     "data_class": "real", "from_format": "goat-processors-system-track", "in_frame": "newline",
     "outputs": [{"id": "cat062", "label": "ASTERIX CAT062", "to_format": "asterix-cat062",
                  "set": {"cat062.sac": "38", "cat062.sic": "204"}},
                 {"id": "sapient", "to_format": "sapient"}]},
    {"id": "sim", "key": "slot/sim/tracks", "kind": "processed", "data_class": "synthetic",
     "from_format": "goat-processors-system-track", "outputs": [{"id": "cat062", "to_format": "asterix-cat062"}]}
  ]}`

func mustConfig(t *testing.T) *Config {
	t.Helper()
	c, err := ParseConfig([]byte(testConfig))
	if err != nil {
		t.Fatal(err)
	}
	return c
}

// memReg is an in-memory Registry for tests.
type memReg struct {
	mu     sync.Mutex
	feeds  map[string]Feed
	events []Event
	n      int
}

func newMem() *memReg { return &memReg{feeds: map[string]Feed{}} }

func (m *memReg) Create(_ context.Context, f Feed, actor, detail string) (Feed, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.n++
	f.ID = "feed-" + string(rune('a'+m.n))
	f.CreatedAt, f.UpdatedAt = time.Now(), time.Now()
	m.feeds[f.ID] = f
	m.events = append(m.events, Event{FeedID: f.ID, Actor: actor, Action: "request", Detail: detail})
	return f, nil
}
func (m *memReg) Get(_ context.Context, id string) (Feed, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	f, ok := m.feeds[id]
	if !ok {
		return f, ErrNotFound
	}
	return f, nil
}
func (m *memReg) List(_ context.Context, statuses ...string) ([]Feed, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	out := []Feed{}
	for _, f := range m.feeds {
		if len(statuses) == 0 || contains(statuses, f.Status) {
			out = append(out, f)
		}
	}
	return out, nil
}
func contains(xs []string, x string) bool {
	for _, v := range xs {
		if v == x {
			return true
		}
	}
	return false
}
func (m *memReg) PortInUse(_ context.Context, ip string, port int, tr string) (bool, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	for _, f := range m.feeds {
		if f.DestIP == ip && f.Port == port && f.Transport == tr && contains(liveStates, f.Status) {
			return true, nil
		}
	}
	return false, nil
}
func (m *memReg) Transition(_ context.Context, id, status, actor, action, note string, decision bool, endAt *time.Time) (Feed, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	f, ok := m.feeds[id]
	if !ok {
		return f, ErrNotFound
	}
	f.Status = status
	if decision {
		f.DecidedBy, f.DecisionNote = actor, note
	}
	if endAt != nil {
		f.EndAt = *endAt
	}
	m.feeds[id] = f
	m.events = append(m.events, Event{FeedID: id, Actor: actor, Action: action, Detail: note})
	return f, nil
}
func (m *memReg) Events(_ context.Context, id string, _ int) ([]Event, error) { return m.events, nil }
func (m *memReg) ExpireDue(ctx context.Context, now time.Time) ([]string, error) {
	var ids []string
	for _, f := range m.feeds {
		if contains(liveStates, f.Status) && f.EndAt.Before(now) {
			ids = append(ids, f.ID)
		}
	}
	for _, id := range ids {
		_, _ = m.Transition(ctx, id, "expired", "goat-feeds", "expire", "", false, nil)
	}
	return ids, nil
}
func (m *memReg) Ping(context.Context) error { return nil }

func req(h map[string]string) *http.Request {
	r, _ := http.NewRequest("GET", "/", nil)
	for k, v := range h {
		r.Header.Set(k, v)
	}
	return r
}

func TestViewerRoles(t *testing.T) {
	c := mustConfig(t)
	if ViewerFromRequest(req(nil), c) != nil {
		t.Fatal("no email must mean no viewer")
	}
	cases := map[string]string{"PLATFORM-ADMIN": RolePlatform, "TEST-TACTICAL-ADMIN": RoleTactical, "DEVELOPER": RoleMember,
		"DEVELOPER, PLATFORM-ADMIN": RolePlatform}
	for groups, want := range cases {
		v := ViewerFromRequest(req(map[string]string{"X-Forwarded-Email": "a@x", "X-Forwarded-Groups": groups}), c)
		if v.Role != want {
			t.Errorf("groups %q: role %s, want %s", groups, v.Role, want)
		}
	}
	// The forgeable header is never read.
	v := ViewerFromRequest(req(map[string]string{"X-Forwarded-Email": "a@x", "X-Auth-Request-Groups": "PLATFORM-ADMIN"}), c)
	if v.Role != RoleMember {
		t.Error("X-Auth-Request-Groups must not grant a role")
	}
}

func good() FeedRequest {
	return FeedRequest{TopicID: "fused", OutputID: "cat062", DestName: "nav-1", DestIP: "100.64.0.9", Port: float64(7005),
		Transport: "udp", Days: float64(30), Purpose: "air picture to the console"}
}

func TestValidateFirstState(t *testing.T) {
	c, ctx, now := mustConfig(t), context.Background(), time.Now()
	member := &Viewer{Email: "m@x", Role: RoleMember}
	tac := &Viewer{Email: "t@x", Role: RoleTactical, IsAdmin: true}
	f, err := ValidateRequest(ctx, good(), member, c, newMem(), now)
	if err != nil || f.Status != "pending" {
		t.Fatalf("member, real topic: %v %s; want pending", err, f.Status)
	}
	f, err = ValidateRequest(ctx, good(), tac, c, newMem(), now)
	if err != nil || f.Status != "active" || f.DecidedBy != "t@x" {
		t.Fatalf("tactical admin: %v %+v; want active, decided by requester", err, f)
	}
	sim := good()
	sim.TopicID = "sim"
	f, err = ValidateRequest(ctx, sim, member, c, newMem(), now)
	if err != nil || f.Status != "active" {
		t.Fatalf("member, synthetic topic: %v %s; want active (self-service)", err, f.Status)
	}
	c.SelfServiceSynthetic = false
	f, _ = ValidateRequest(ctx, sim, member, c, newMem(), now)
	if f.Status != "pending" {
		t.Fatal("self-service off: synthetic must wait")
	}
}

func TestValidateRefusals(t *testing.T) {
	c, ctx := mustConfig(t), context.Background()
	v := &Viewer{Email: "m@x", Role: RoleMember}
	mutate := map[string]func(*FeedRequest){
		"topic":     func(r *FeedRequest) { r.TopicID = "nope" },
		"output":    func(r *FeedRequest) { r.OutputID = "nope" },
		"name":      func(r *FeedRequest) { r.DestName = "" },
		"ip":        func(r *FeedRequest) { r.DestIP = "not-an-ip" },
		"mesh":      func(r *FeedRequest) { r.DestIP = "192.168.1.5" },
		"port":      func(r *FeedRequest) { r.Port = float64(80) },
		"transport": func(r *FeedRequest) { r.Transport = "sctp" },
		"days":      func(r *FeedRequest) { r.Days = float64(999) },
		"purpose":   func(r *FeedRequest) { r.Purpose = "x" },
	}
	for name, m := range mutate {
		r := good()
		m(&r)
		if _, err := ValidateRequest(ctx, r, v, c, newMem(), time.Now()); err == nil {
			t.Errorf("%s: want a refusal", name)
		}
	}
	reg := newMem()
	f, _ := ValidateRequest(ctx, good(), v, c, reg, time.Now())
	_, _ = reg.Create(ctx, f, v.Email, "")
	if _, err := ValidateRequest(ctx, good(), v, c, reg, time.Now()); err == nil || err.(*Refused).Status != 409 {
		t.Errorf("second feed to the same port: %v; want 409", err)
	}
}

func TestCan(t *testing.T) {
	owner := &Viewer{Email: "o@x", Role: RoleMember}
	other := &Viewer{Email: "z@x", Role: RoleMember}
	admin := &Viewer{Email: "a@x", Role: RoleTactical, IsAdmin: true}
	pending := Feed{Owner: "o@x", Status: "pending"}
	active := Feed{Owner: "o@x", Status: "active"}
	if Can(owner, "approve", pending) || !Can(admin, "approve", pending) {
		t.Error("only an admin approves")
	}
	if !Can(owner, "pause", active) || Can(other, "pause", active) || !Can(admin, "pause", active) {
		t.Error("owner or admin pauses")
	}
	if Can(owner, "renew", active) || !Can(admin, "renew", active) {
		t.Error("only an admin renews")
	}
}

func TestFeedHealth(t *testing.T) {
	now := time.Now()
	run := &GroupView{Running: true, StatsAt: now, LastInChange: now, Stats: Stats{In: 10, Converted: 10}}
	if h, _ := feedHealth(Feed{Status: "active", Transport: "udp"}, run, false); h != "sending" {
		t.Errorf("flowing udp: %s", h)
	}
	stale := *run
	stale.LastInChange = now.Add(-5 * time.Minute)
	if h, _ := feedHealth(Feed{Status: "active"}, &stale, false); h != "quiet" {
		t.Errorf("no input for 5 min: %s", h)
	}
	dead := &GroupView{Running: false, LastError: "the pipe exited"}
	if h, note := feedHealth(Feed{Status: "active"}, dead, false); h != "broken" || !strings.Contains(note, "exited") {
		t.Errorf("dead process: %s %s", h, note)
	}
	bad := &GroupView{Running: true, StatsAt: now, LastInChange: now, Stats: Stats{In: 5, Failed: 5}, LastError: "no kind"}
	if h, _ := feedHealth(Feed{Status: "active"}, bad, false); h != "broken" {
		t.Errorf("nothing converts: %s", h)
	}
}

// fakeProc is a process whose output the test writes.
type fakeProc struct {
	r    *io.PipeReader
	w    *io.PipeWriter
	argv []string
	once sync.Once
}

func (p *fakeProc) Output() io.Reader { return p.r }
func (p *fakeProc) Wait() error       { return nil }
func (p *fakeProc) Pid() int          { return 1 }
func (p *fakeProc) Stop()             { p.once.Do(func() { _ = p.w.Close() }) }

type fakeSpawner struct {
	mu    sync.Mutex
	procs []*fakeProc
}

func (s *fakeSpawner) spawn(argv []string) (Process, error) {
	r, w := io.Pipe()
	p := &fakeProc{r: r, w: w, argv: argv}
	s.mu.Lock()
	s.procs = append(s.procs, p)
	s.mu.Unlock()
	return p, nil
}

func TestSupervisorGroupsAndFansOut(t *testing.T) {
	c, ctx := mustConfig(t), context.Background()
	reg := newMem()
	sp := &fakeSpawner{}
	sup := NewSupervisor(c, reg, sp.spawn, time.Hour)
	mk := func(ip string, port int, out string) Feed {
		f, _ := reg.Create(ctx, Feed{TopicID: "fused", OutputID: out, DestIP: ip, Port: port, Transport: "udp",
			Status: "active", EndAt: time.Now().Add(time.Hour)}, "t", "")
		return f
	}
	a := mk("198.51.100.1", 7005, "cat062")
	mk("198.51.100.2", 7005, "cat062")
	mk("198.51.100.3", 9000, "sapient")
	if err := sup.Converge(ctx); err != nil {
		t.Fatal(err)
	}
	if n := len(sp.procs); n != 2 {
		t.Fatalf("three feeds on two output forms: %d processes, want 2", n)
	}
	var cat *fakeProc
	for _, p := range sp.procs {
		if strings.Contains(strings.Join(p.argv, " "), "asterix-cat062") {
			cat = p
		}
	}
	joined := strings.Join(cat.argv, " ")
	for _, want := range []string{"--out udp://198.51.100.1:7005", "--out udp://198.51.100.2:7005", "--set cat062.sac=38", "zenoh:slot/fused/tracks"} {
		if !strings.Contains(joined, want) {
			t.Errorf("command lacks %q: %s", want, joined)
		}
	}
	// Counters reach the feed view.
	go func() { _, _ = io.WriteString(cat.w, "goat-xf-pipe-zenoh: in=10 converted=20 failed=0 10s\n") }()
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		if v := sup.FeedView(a.ID); v != nil && v.Stats.In == 10 {
			if v.SentEstimate != 10 || v.SharedWith != 1 {
				t.Errorf("sent estimate %d shared %d; want 10 and 1", v.SentEstimate, v.SharedWith)
			}
			break
		}
		time.Sleep(10 * time.Millisecond)
	}
	// Pausing one feed restarts only its group, with one output fewer.
	_, _ = reg.Transition(ctx, a.ID, "paused", "t", "pause", "", false, nil)
	_ = sup.Converge(ctx)
	if n := len(sp.procs); n != 3 {
		t.Fatalf("after a pause: %d spawns, want 3 (one restart)", n)
	}
	if strings.Contains(strings.Join(sp.procs[2].argv, " "), "198.51.100.1") {
		t.Error("the paused feed's output is still in the command")
	}
	if s := sup.Summary(); s.Groups != 2 {
		t.Errorf("groups %d, want 2", s.Groups)
	}
}

func TestExpiry(t *testing.T) {
	ctx, reg := context.Background(), newMem()
	f, _ := reg.Create(ctx, Feed{Status: "active", EndAt: time.Now().Add(-time.Minute)}, "t", "")
	ids, _ := reg.ExpireDue(ctx, time.Now())
	got, _ := reg.Get(ctx, f.ID)
	if len(ids) != 1 || got.Status != "expired" {
		t.Errorf("past end date: %v %s; want expired", ids, got.Status)
	}
}
