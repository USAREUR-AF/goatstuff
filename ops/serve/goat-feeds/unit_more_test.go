// SPDX-License-Identifier: Apache-2.0

package main

import (
	"context"
	"errors"
	"strings"
	"testing"
	"time"
)

// ---- configuration ----

func TestParseConfigRefusals(t *testing.T) {
	cases := map[string]string{
		"no site":           `{"catalog":[]}`,
		"bad ports":         `{"site":"s","port_min":2000,"port_max":1000}`,
		"bad days":          `{"site":"s","default_days":200,"max_days":100}`,
		"bad cidr":          `{"site":"s","allowed_cidr":"nope"}`,
		"topic no key":      `{"site":"s","catalog":[{"id":"a","from_format":"x","kind":"raw","data_class":"real","outputs":[{"id":"o","to_format":"y"}]}]}`,
		"duplicate topic":   `{"site":"s","catalog":[{"id":"a","key":"k","from_format":"x","kind":"raw","data_class":"real","outputs":[{"id":"o","to_format":"y"}]},{"id":"a","key":"k2","from_format":"x","kind":"raw","data_class":"real","outputs":[{"id":"o","to_format":"y"}]}]}`,
		"bad kind":          `{"site":"s","catalog":[{"id":"a","key":"k","from_format":"x","kind":"nope","data_class":"real","outputs":[{"id":"o","to_format":"y"}]}]}`,
		"bad class":         `{"site":"s","catalog":[{"id":"a","key":"k","from_format":"x","kind":"raw","data_class":"nope","outputs":[{"id":"o","to_format":"y"}]}]}`,
		"no outputs":        `{"site":"s","catalog":[{"id":"a","key":"k","from_format":"x","kind":"raw","data_class":"real","outputs":[]}]}`,
		"output no format":  `{"site":"s","catalog":[{"id":"a","key":"k","from_format":"x","kind":"raw","data_class":"real","outputs":[{"id":"o"}]}]}`,
		"intake bad ip":     `{"site":"s","intake":{"listen_ip":"nope","ports":[1],"key_prefix":"p","formats":[{"id":"f","from_format":"x"}],"outputs":[{"id":"o","to_format":"y"}]}}`,
		"intake no formats": `{"site":"s","intake":{"listen_ip":"198.51.100.9","ports":[1],"key_prefix":"p","formats":[],"outputs":[{"id":"o","to_format":"y"}]}}`,
		"intake no prefix":  `{"site":"s","intake":{"listen_ip":"198.51.100.9","ports":[1],"formats":[{"id":"f","from_format":"x"}],"outputs":[{"id":"o","to_format":"y"}]}}`,
		"intake format bad": `{"site":"s","intake":{"listen_ip":"198.51.100.9","ports":[1],"key_prefix":"p","formats":[{"id":"f"}],"outputs":[{"id":"o","to_format":"y"}]}}`,
		"intake output bad": `{"site":"s","intake":{"listen_ip":"198.51.100.9","ports":[1],"key_prefix":"p","formats":[{"id":"f","from_format":"x"}],"outputs":[{"id":"o"}]}}`,
		"not json":          `{`,
	}
	for name, cfg := range cases {
		if _, err := ParseConfig([]byte(cfg)); err == nil {
			t.Errorf("%s: accepted", name)
		}
	}
}

func TestParseConfigDefaults(t *testing.T) {
	c, err := ParseConfig([]byte(`{"site":"s","catalog":[{"id":"a","key":"k","from_format":"x","kind":"raw","data_class":"real","outputs":[{"id":"o","to_format":"y"}]}],
	  "intake":{"listen_ip":"198.51.100.9","ports":[7100],"key_prefix":"p/","formats":[{"id":"f","from_format":"x"}],"outputs":[{"id":"o","to_format":"y"}]}}`))
	if err != nil {
		t.Fatal(err)
	}
	if c.Goatnet != "s" || c.Listen != "127.0.0.1:8098" || c.Catalog[0].Label != "a" || c.Catalog[0].Outputs[0].Label != "o" {
		t.Errorf("defaults not applied: %+v", c)
	}
	if c.Intake.ToFormat != "goat-processors-system-track" || c.Intake.OutFrame != "newline" || c.Intake.Formats[0].Label != "f" || c.Intake.Outputs[0].Label != "o" {
		t.Errorf("intake defaults not applied: %+v", c.Intake)
	}
	if PublishedKey(c, Feed{ID: "feed-1"}) != "p/feed-1" {
		t.Errorf("published key: %s", PublishedKey(c, Feed{ID: "feed-1"}))
	}
	// An empty intake block means the site receives nothing.
	c2, err := ParseConfig([]byte(`{"site":"s","intake":{}}`))
	if err != nil || c2.Intake.Enabled() {
		t.Errorf("empty intake: %v enabled=%v", err, c2.Intake.Enabled())
	}
	var nilIntake *Intake
	if nilIntake.Enabled() {
		t.Error("a nil intake is enabled")
	}
}

// ---- permissions: every action, role and state ----

func TestCanMatrix(t *testing.T) {
	member := &Viewer{Email: "m@x", Role: RoleMember}
	owner := &Viewer{Email: "o@x", Role: RoleMember}
	tactical := &Viewer{Email: "t@x", Role: RoleTactical, IsAdmin: true}
	platform := &Viewer{Email: "p@x", Role: RolePlatform, IsAdmin: true}
	states := []string{"pending", "active", "paused", "declined", "expired", "removed"}
	// want[action][state] for (other member, owner, admin)
	type who struct{ other, own, admin bool }
	want := map[string]map[string]who{
		"approve": {"pending": {false, false, true}},
		"decline": {"pending": {false, false, true}},
		"pause":   {"active": {false, true, true}},
		"resume":  {"paused": {false, true, true}},
		"renew":   {"active": {false, false, true}, "paused": {false, false, true}, "expired": {false, false, true}},
		"remove":  {"pending": {false, true, true}, "active": {false, true, true}, "paused": {false, true, true}, "declined": {false, true, true}, "expired": {false, true, true}},
		"check":   {"pending": {true, true, true}, "active": {true, true, true}, "paused": {true, true, true}, "declined": {true, true, true}, "expired": {true, true, true}, "removed": {true, true, true}},
	}
	for _, action := range allActions {
		for _, st := range states {
			f := Feed{Owner: "O@X", Status: st} // owner match is case-insensitive
			w := want[action][st]
			if got := Can(member, action, f); got != w.other {
				t.Errorf("%s/%s other member: %v, want %v", action, st, got, w.other)
			}
			if got := Can(owner, action, f); got != w.own {
				t.Errorf("%s/%s owner: %v, want %v", action, st, got, w.own)
			}
			for _, a := range []*Viewer{tactical, platform} {
				if got := Can(a, action, f); got != w.admin {
					t.Errorf("%s/%s %s: %v, want %v", action, st, a.Role, got, w.admin)
				}
			}
		}
	}
	if Can(platform, "nope", Feed{Status: "active"}) {
		t.Error("an unknown action is allowed")
	}
	if got := ActionsFor(owner, Feed{Owner: "o@x", Status: "active"}); strings.Join(got, ",") != "pause,remove,check" {
		t.Errorf("owner's actions on an active feed: %v", got)
	}
}

// ---- validation details ----

func TestValidateDetails(t *testing.T) {
	c, ctx := mustConfig(t), context.Background()
	admin := &Viewer{Email: "a@x", Role: RolePlatform, IsAdmin: true}
	base := good()
	mut := func(f func(*FeedRequest)) FeedRequest { r := base; f(&r); return r }
	refused := map[string]FeedRequest{
		"unknown topic":  mut(func(r *FeedRequest) { r.TopicID = "nope" }),
		"unknown output": mut(func(r *FeedRequest) { r.OutputID = "nope" }),
		"bad name":       mut(func(r *FeedRequest) { r.DestName = "<script>" }),
		"not an ip":      mut(func(r *FeedRequest) { r.DestIP = "host.example" }),
		"off mesh":       mut(func(r *FeedRequest) { r.DestIP = "203.0.113.5" }),
		"port text":      mut(func(r *FeedRequest) { r.Port = "abc" }),
		"port low":       mut(func(r *FeedRequest) { r.Port = float64(80) }),
		"port fraction":  mut(func(r *FeedRequest) { r.Port = 7005.5 }),
		"bad transport":  mut(func(r *FeedRequest) { r.Transport = "sctp" }),
		"days text":      mut(func(r *FeedRequest) { r.Days = "soon" }),
		"days zero":      mut(func(r *FeedRequest) { r.Days = float64(0) }),
		"days too many":  mut(func(r *FeedRequest) { r.Days = float64(181) }),
		"short purpose":  mut(func(r *FeedRequest) { r.Purpose = "x" }),
		"long purpose":   mut(func(r *FeedRequest) { r.Purpose = strings.Repeat("x", 501) }),
		"bad direction":  mut(func(r *FeedRequest) { r.Direction = "sideways" }),
	}
	for name, r := range refused {
		_, err := ValidateRequest(ctx, r, admin, c, newMem(), time.Now())
		var ref *Refused
		if !errors.As(err, &ref) {
			t.Errorf("%s: %v, want a refusal", name, err)
		}
	}
	// Defaults: transport udp, days from config, port as a string.
	f, err := ValidateRequest(ctx, mut(func(r *FeedRequest) { r.Transport = ""; r.Days = nil; r.Port = "7005" }), admin, c, newMem(), time.Now())
	if err != nil || f.Transport != "udp" || f.Direction != "out" || f.EndAt.Sub(time.Now()) < 29*24*time.Hour {
		t.Errorf("defaults: %+v %v", f, err)
	}
}

func TestIntakeValidationDetails(t *testing.T) {
	c, ctx := mustConfig(t), context.Background()
	admin := &Viewer{Email: "a@x", Role: RolePlatform, IsAdmin: true}
	base := FeedRequest{Direction: "in", FormatID: "cot", Label: "Blue drone", DestName: "console-west", DestIP: "198.51.100.42",
		Port: float64(7101), Purpose: "position into the picture"}
	mut := func(f func(*FeedRequest)) FeedRequest { r := base; f(&r); return r }
	refused := map[string]FeedRequest{
		"no label":      mut(func(r *FeedRequest) { r.Label = "" }),
		"bad sender":    mut(func(r *FeedRequest) { r.DestName = "" }),
		"not an ip":     mut(func(r *FeedRequest) { r.DestIP = "x" }),
		"off mesh":      mut(func(r *FeedRequest) { r.DestIP = "203.0.113.5" }),
		"port text":     mut(func(r *FeedRequest) { r.Port = "x" }),
		"tcp":           mut(func(r *FeedRequest) { r.Transport = "tcp" }),
		"days":          mut(func(r *FeedRequest) { r.Days = float64(999) }),
		"short purpose": mut(func(r *FeedRequest) { r.Purpose = "" }),
	}
	for name, r := range refused {
		if _, err := ValidateRequest(ctx, r, admin, c, newMem(), time.Now()); err == nil {
			t.Errorf("%s: accepted", name)
		}
	}
	noIntake, _ := ParseConfig([]byte(`{"site":"s"}`))
	if _, err := ValidateRequest(ctx, base, admin, noIntake, newMem(), time.Now()); err == nil || !strings.Contains(err.Error(), "does not receive") {
		t.Errorf("site without intake: %v", err)
	}
}

// ---- health words ----

func TestFeedHealthBranches(t *testing.T) {
	now := time.Now()
	cases := []struct {
		f    Feed
		d    *GroupView
		gone bool
		want string
	}{
		{Feed{Status: "pending"}, nil, false, "waiting"},
		{Feed{Status: "paused"}, nil, false, "paused"},
		{Feed{Status: "declined"}, nil, false, "declined"},
		{Feed{Status: "expired"}, nil, false, "expired"},
		{Feed{Status: "removed"}, nil, false, "removed"},
		{Feed{Status: "active"}, nil, true, "broken"},
		{Feed{Status: "active"}, nil, false, "starting"},
		{Feed{Status: "active"}, &GroupView{Running: true}, false, "starting"},
		{Feed{Status: "active"}, &GroupView{Running: true, StatsAt: now}, false, "quiet"},
		{Feed{Status: "active", Transport: "tcp"}, &GroupView{Running: true, StatsAt: now, LastInChange: now, Stats: Stats{In: 1, Converted: 1}}, false, "sending"},
	}
	for i, c := range cases {
		if got, note := feedHealth(c.f, c.d, c.gone); got != c.want {
			t.Errorf("case %d: %s (%s), want %s", i, got, note, c.want)
		}
	}
	if _, note := feedHealth(Feed{Status: "active", Transport: "udp"}, &GroupView{Running: true, StatsAt: now, LastInChange: now, Stats: Stats{In: 1, Converted: 1}}, false); !strings.Contains(note, "UDP") {
		t.Errorf("udp sending note: %s", note)
	}
}

func TestIntakeHealthBranches(t *testing.T) {
	now := time.Now()
	cases := []struct {
		f    Feed
		d    *GroupView
		gone bool
		want string
	}{
		{Feed{Status: "pending"}, nil, false, "waiting"},
		{Feed{Status: "paused"}, nil, false, "paused"},
		{Feed{Status: "removed"}, nil, false, "removed"},
		{Feed{Status: "active"}, nil, true, "broken"},
		{Feed{Status: "active"}, nil, false, "starting"},
		{Feed{Status: "active"}, &GroupView{Running: false, LastError: "x"}, false, "broken"},
		{Feed{Status: "active"}, &GroupView{Running: true, StatsAt: now, LastInChange: now.Add(-2 * time.Minute), Stats: Stats{In: 3, Converted: 3}}, false, "quiet"},
	}
	for i, c := range cases {
		if got, note := intakeHealth(c.f, c.d, c.gone); got != c.want {
			t.Errorf("case %d: %s (%s), want %s", i, got, note, c.want)
		}
	}
}

// ---- commands ----

func TestPipeCommandArgv(t *testing.T) {
	c := mustConfig(t)
	topic := c.Topic("fused")
	out := topic.Output("cat062")
	argv := pipeCommand(c, topic, out, []string{"udp://198.51.100.1:7005", "tcp://198.51.100.2:9000"})
	want := []string{c.Pipe.Binary, "--packs", c.Pipe.Packs, "--packs-only", "--from-format", "goat-processors-system-track",
		"--in", "zenoh:slot/fused/tracks?config=" + c.Pipe.ZenohConfig, "--in-frame", "newline",
		"--set", "cat062.sac=38", "--set", "cat062.sic=204", "--to-format", "asterix-cat062",
		"--out", "udp://198.51.100.1:7005", "--out", "tcp://198.51.100.2:9000"}
	if strings.Join(argv, " ") != strings.Join(want, " ") {
		t.Errorf("argv\n got %v\nwant %v", argv, want)
	}
	// --set keys are sorted, so a config reorder does not restart a group.
	if groupKey(topic, out) != groupKey(topic, &Output{ID: "cat062", ToFormat: "asterix-cat062", Set: map[string]string{"cat062.sic": "204", "cat062.sac": "38"}}) {
		t.Error("group key depends on map order")
	}
}

func TestIntakeCommandArgv(t *testing.T) {
	c := mustConfig(t)
	f := Feed{ID: "feed-x", Port: 7101}
	ft := &IntakeFormat{ID: "z", FromFormat: "cot-xml", InFrame: "newline", Set: map[string]string{"b": "2", "a": "1"}}
	got := strings.Join(intakeCommand(c, f, ft), " ")
	for _, w := range []string{"--from-format cot-xml", "--in udp://198.51.100.9:7101", "--in-frame newline", "--set a=1 --set b=2",
		"--set track.source_id=feed-x", "--to-format goat-processors-system-track", "--out-frame newline",
		"--out zenoh:volatile/slot/stream/feeds-in/feed-x?config="} {
		if !strings.Contains(got, w) {
			t.Errorf("intake argv lacks %q: %s", w, got)
		}
	}
}

// ---- supervisor ----

func TestSupervisorRestartsWithBackoff(t *testing.T) {
	c, ctx, reg := mustConfig(t), context.Background(), newMem()
	sp := &fakeSpawner{}
	sup := NewSupervisor(c, reg, sp.spawn, time.Hour)
	f, _ := reg.Create(ctx, Feed{TopicID: "fused", OutputID: "cat062", DestIP: "198.51.100.1", Port: 7005, Transport: "udp",
		Status: "active", EndAt: time.Now().Add(time.Hour)}, "t", "")
	_ = sup.Converge(ctx)
	sp.procs[0].Stop() // the pipe exits
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		if v := sup.FeedView(f.ID); v != nil && !v.Running {
			break
		}
		time.Sleep(10 * time.Millisecond)
	}
	v := sup.FeedView(f.ID)
	if v == nil || v.Running || !strings.Contains(v.LastError, "the pipe") {
		t.Fatalf("an exited pipe is not shown as stopped: %+v", v)
	}
	if h, _ := feedHealth(f, v, false); h != "broken" {
		t.Errorf("health of a stopped pipe: %s", h)
	}
	_ = sup.Converge(ctx) // still inside the back-off window
	if len(sp.procs) != 1 {
		t.Errorf("restarted inside the back-off: %d spawns", len(sp.procs))
	}
	if backoff(0) != time.Second || backoff(3) != 8*time.Second || backoff(20) != time.Minute {
		t.Errorf("backoff: %v %v %v", backoff(0), backoff(3), backoff(20))
	}
	if s := sup.Summary(); s.Groups != 1 || s.Running != 0 {
		t.Errorf("summary after exit: %+v", s)
	}
	m := sup.Metrics()
	if len(m) != 5 || m[0].labels["feed"] != f.ID {
		t.Errorf("metrics rows: %+v", m)
	}
	sup.StopAll()
}

func TestSupervisorDropsFeedsWithoutTopic(t *testing.T) {
	c, ctx, reg := mustConfig(t), context.Background(), newMem()
	sp := &fakeSpawner{}
	sup := NewSupervisor(c, reg, sp.spawn, time.Hour)
	_, _ = reg.Create(ctx, Feed{TopicID: "gone", OutputID: "x", Status: "active", EndAt: time.Now().Add(time.Hour)}, "t", "")
	_, _ = reg.Create(ctx, Feed{TopicID: "fused", OutputID: "gone", Status: "active", EndAt: time.Now().Add(time.Hour)}, "t", "")
	_, _ = reg.Create(ctx, Feed{TopicID: "intake", OutputID: "gone", Direction: "in", Status: "active", EndAt: time.Now().Add(time.Hour)}, "t", "")
	if err := sup.Converge(ctx); err != nil {
		t.Fatal(err)
	}
	if len(sp.procs) != 0 {
		t.Errorf("feeds with no topic, output or format started %d processes", len(sp.procs))
	}
}

func TestSupervisorRunExpiresAndStops(t *testing.T) {
	c, reg := mustConfig(t), newMem()
	sp := &fakeSpawner{}
	sup := NewSupervisor(c, reg, sp.spawn, 10*time.Millisecond)
	ctx, cancel := context.WithCancel(context.Background())
	f, _ := reg.Create(ctx, Feed{TopicID: "fused", OutputID: "cat062", DestIP: "198.51.100.1", Port: 7005, Transport: "udp",
		Status: "active", EndAt: time.Now().Add(-time.Second)}, "t", "")
	done := make(chan struct{})
	go func() { sup.Run(ctx); close(done) }()
	time.Sleep(60 * time.Millisecond)
	cancel()
	<-done
	got, _ := reg.Get(context.Background(), f.ID)
	if got.Status != "expired" {
		t.Errorf("past its end date: %s, want expired", got.Status)
	}
	if len(sp.procs) != 0 {
		t.Errorf("an expired feed started %d processes", len(sp.procs))
	}
	if sup.Summary().LastConverge.IsZero() {
		t.Error("the loop never converged")
	}
}
