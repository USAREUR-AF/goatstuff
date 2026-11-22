// SPDX-License-Identifier: Apache-2.0

package main

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"os/exec"
	"regexp"
	"sort"
	"strconv"
	"sync"
	"syscall"
	"time"
)

// Delivery: one goat-xf-pipe-zenoh process per (topic, output form), one --out per feed.
// The supervisor converges running processes on the active feeds every few seconds (ADR 1109).
// Fabric load is one subscription per group, not per feed (ADR 1123).

var (
	statsRE = regexp.MustCompile(`in=(\d+)\s+converted=(\d+)\s+failed=(\d+)`)
	errorRE = regexp.MustCompile(`(?i)(error|fail|refus|denied|cannot|unable)`)
)

func groupKey(t *Topic, o *Output) string {
	keys := make([]string, 0, len(o.Set))
	for k := range o.Set {
		keys = append(keys, k+"="+o.Set[k])
	}
	sort.Strings(keys)
	b, _ := json.Marshal([]any{t.Key, t.FromFormat, t.InFrame, o.ToFormat, o.Frame, keys})
	return string(b)
}

func outURI(f Feed) string { return fmt.Sprintf("%s://%s:%d", f.Transport, f.DestIP, f.Port) }

// intakeCommand receives one system's datagrams on this core node and publishes each message,
// converted to the site's canonical form, on the fabric.
func intakeCommand(cfg *Config, f Feed, ft *IntakeFormat) []string {
	in := cfg.Intake
	cmd := []string{cfg.Pipe.Binary, "--packs", cfg.Pipe.Packs, "--packs-only", "--from-format", ft.FromFormat,
		"--in", fmt.Sprintf("udp://%s:%d", in.ListenIP, f.Port)}
	if ft.InFrame != "" {
		cmd = append(cmd, "--in-frame", ft.InFrame)
	}
	keys := make([]string, 0, len(ft.Set))
	for k := range ft.Set {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	for _, k := range keys {
		cmd = append(cmd, "--set", k+"="+ft.Set[k])
	}
	cmd = append(cmd, "--set", "track.source_id="+f.ID, "--to-format", in.ToFormat, "--out-frame", in.OutFrame,
		"--out", "zenoh:"+PublishedKey(cfg, f)+"?config="+cfg.Pipe.ZenohConfig)
	return cmd
}

// publishedTopic is a live received feed offered like a catalog topic.
func publishedTopic(cfg *Config, f Feed) Topic {
	return Topic{ID: "in-" + f.ID, Label: f.Label, Key: PublishedKey(cfg, f), Kind: "published", DataClass: "real",
		FromFormat: cfg.Intake.ToFormat, InFrame: cfg.Intake.OutFrame,
		Description: fmt.Sprintf("Received from %s (%s), published by goat-feeds.", f.DestName, f.DestIP),
		Outputs:     cfg.Intake.Outputs}
}

func pipeCommand(cfg *Config, t *Topic, o *Output, outs []string) []string {
	cmd := []string{cfg.Pipe.Binary, "--packs", cfg.Pipe.Packs, "--packs-only", "--from-format", t.FromFormat,
		"--in", "zenoh:" + t.Key + "?config=" + cfg.Pipe.ZenohConfig}
	if t.InFrame != "" {
		cmd = append(cmd, "--in-frame", t.InFrame)
	}
	keys := make([]string, 0, len(o.Set))
	for k := range o.Set {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	for _, k := range keys {
		cmd = append(cmd, "--set", k+"="+o.Set[k])
	}
	cmd = append(cmd, "--to-format", o.ToFormat)
	if o.Frame != "" {
		cmd = append(cmd, "--out-frame", o.Frame)
	}
	for _, u := range outs {
		cmd = append(cmd, "--out", u)
	}
	return cmd
}

// Process is what the supervisor needs from a running pipe; tests replace it.
type Process interface {
	Output() io.Reader
	Wait() error
	Stop()
	Pid() int
}

type Spawner func(argv []string) (Process, error)

type execProcess struct {
	cmd  *exec.Cmd
	out  *os.File
	once sync.Once
	done chan struct{}
	err  error
}

func (p *execProcess) Output() io.Reader { return p.out }
func (p *execProcess) Pid() int          { return p.cmd.Process.Pid }

// Wait is called once, by the reader, after the output reaches EOF.
func (p *execProcess) Wait() error {
	p.once.Do(func() { p.err = p.cmd.Wait(); _ = p.out.Close(); close(p.done) })
	return p.err
}

func (p *execProcess) Stop() {
	_ = p.cmd.Process.Signal(syscall.SIGTERM)
	select {
	case <-p.done:
	case <-time.After(5 * time.Second):
		_ = p.cmd.Process.Kill()
	}
}

func ExecSpawner(argv []string) (Process, error) {
	r, w, err := os.Pipe()
	if err != nil {
		return nil, err
	}
	cmd := exec.Command(argv[0], argv[1:]...)
	cmd.Stdout, cmd.Stderr = w, w
	if err := cmd.Start(); err != nil {
		_ = r.Close()
		_ = w.Close()
		return nil, err
	}
	_ = w.Close() // the child holds its own copy; EOF arrives when it exits
	return &execProcess{cmd: cmd, out: r, done: make(chan struct{})}, nil
}

type Stats struct {
	In        int `json:"in"`
	Converted int `json:"converted"`
	Failed    int `json:"failed"`
}

type Group struct {
	Key          string
	Topic        *Topic
	Output       *Output
	Outs         []string
	Feeds        []string
	Cmd          []string
	proc         Process
	running      bool
	StartedAt    time.Time
	Restarts     int
	LastExit     string
	backoffUntil time.Time
	Lines        []string
	LastError    string
	LastErrorAt  time.Time
	Stats        Stats
	StatsAt      time.Time
	LastInChange time.Time
	gen          int
}

type GroupView struct {
	Running      bool      `json:"running"`
	Pid          int       `json:"pid,omitempty"`
	StartedAt    time.Time `json:"started_at"`
	Restarts     int       `json:"restarts"`
	LastExit     string    `json:"last_exit"`
	Outs         []string  `json:"outs"`
	Feeds        []string  `json:"feeds"`
	Command      []string  `json:"command"`
	Stats        Stats     `json:"stats"`
	StatsAt      time.Time `json:"stats_at"`
	LastInChange time.Time `json:"last_in_change_at"`
	LastError    string    `json:"last_error"`
	LastErrorAt  time.Time `json:"last_error_at"`
	Lines        []string  `json:"lines"`
	SentEstimate int       `json:"sent_estimate"`
	SharedWith   int       `json:"shared_with"`
}

type Supervisor struct {
	cfg      *Config
	reg      Registry
	spawn    Spawner
	interval time.Duration

	mu             sync.Mutex
	groups         map[string]*Group
	LastConverge   time.Time
	LastConvergeEr string
}

func NewSupervisor(cfg *Config, reg Registry, spawn Spawner, interval time.Duration) *Supervisor {
	return &Supervisor{cfg: cfg, reg: reg, spawn: spawn, interval: interval, groups: map[string]*Group{}}
}

type member struct{ feed, out string }

func (s *Supervisor) desired(feeds []Feed) map[string]*Group {
	want := map[string]*Group{}
	members := map[string][]member{}
	for _, f := range feeds {
		if f.Direction == "in" {
			if !s.cfg.Intake.Enabled() {
				continue
			}
			ft := s.cfg.Intake.Format(f.OutputID)
			if ft == nil {
				continue
			}
			k := "in:" + f.ID
			want[k] = &Group{Key: k, Topic: &Topic{ID: "intake", Label: f.Label}, Output: &Output{ID: ft.ID, ToFormat: ft.FromFormat},
				Outs: []string{fmt.Sprintf("udp://%s:%d", s.cfg.Intake.ListenIP, f.Port)}, Feeds: []string{f.ID},
				Cmd: intakeCommand(s.cfg, f, ft)}
			continue
		}
		t := s.cfg.Topic(f.TopicID)
		if t == nil {
			continue
		}
		o := t.Output(f.OutputID)
		if o == nil {
			continue
		}
		k := groupKey(t, o)
		if want[k] == nil {
			want[k] = &Group{Key: k, Topic: t, Output: o}
		}
		members[k] = append(members[k], member{f.ID, outURI(f)})
	}
	for k, ms := range members {
		sort.Slice(ms, func(i, j int) bool { return ms[i].feed < ms[j].feed })
		seen := map[string]bool{}
		for _, m := range ms {
			want[k].Feeds = append(want[k].Feeds, m.feed)
			if !seen[m.out] {
				seen[m.out] = true
				want[k].Outs = append(want[k].Outs, m.out)
			}
		}
		want[k].Cmd = pipeCommand(s.cfg, want[k].Topic, want[k].Output, want[k].Outs)
	}
	return want
}

func sameOuts(a, b []string) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

// Converge brings running processes in line with the active feeds.
func (s *Supervisor) Converge(ctx context.Context) error {
	live, err := s.reg.List(ctx, "active", "paused")
	if err != nil {
		return err
	}
	var feeds []Feed
	var pub []Topic
	for _, f := range live {
		if f.Direction == "in" && s.cfg.Intake.Enabled() {
			pub = append(pub, publishedTopic(s.cfg, f))
		}
		if f.Status == "active" {
			feeds = append(feeds, f)
		}
	}
	// Publish first, so a feed reading a received topic resolves it in this same pass.
	s.cfg.SetPublished(pub)
	want := s.desired(feeds)
	now := time.Now()
	s.mu.Lock()
	defer s.mu.Unlock()
	for k, g := range s.groups {
		if want[k] == nil {
			s.stopLocked(g)
			delete(s.groups, k)
		}
	}
	for k, w := range want {
		g := s.groups[k]
		if g == nil || !sameOuts(g.Outs, w.Outs) {
			if g != nil {
				s.stopLocked(g)
				w.Restarts, w.Lines = g.Restarts, g.Lines
			}
			s.groups[k] = w
			s.startLocked(w)
			continue
		}
		g.Feeds = w.Feeds
		if !g.running && now.After(g.backoffUntil) {
			g.Restarts++
			s.startLocked(g)
		}
	}
	s.LastConverge = now
	return nil
}

func backoff(restarts int) time.Duration {
	if restarts > 6 {
		restarts = 6
	}
	d := time.Duration(1<<restarts) * time.Second
	if d > time.Minute {
		d = time.Minute
	}
	return d
}

func (s *Supervisor) startLocked(g *Group) {
	p, err := s.spawn(g.Cmd)
	if err != nil {
		g.running = false
		g.LastError, g.LastErrorAt = "could not start the pipe: "+err.Error(), time.Now()
		g.backoffUntil = time.Now().Add(backoff(g.Restarts))
		return
	}
	g.gen++
	g.proc, g.running, g.StartedAt, g.LastExit = p, true, time.Now(), ""
	g.Stats = Stats{}
	go s.read(g, p, g.gen)
}

func (s *Supervisor) stopLocked(g *Group) {
	if g.proc != nil && g.running {
		g.gen++ // the reader sees a newer generation and does not record an exit
		p := g.proc
		g.running = false
		go p.Stop()
	}
	g.proc = nil
}

func (s *Supervisor) appendLine(g *Group, line string) {
	g.Lines = append(g.Lines, time.Now().UTC().Format("15:04:05Z")+" "+line)
	if len(g.Lines) > 200 {
		g.Lines = g.Lines[len(g.Lines)-200:]
	}
}

func (s *Supervisor) read(g *Group, p Process, gen int) {
	sc := bufio.NewScanner(p.Output())
	sc.Buffer(make([]byte, 64*1024), 1024*1024)
	for sc.Scan() {
		line := sc.Text()
		if line == "" {
			continue
		}
		s.mu.Lock()
		if g.gen == gen {
			s.appendLine(g, line)
			if m := statsRE.FindStringSubmatch(line); m != nil {
				in, _ := strconv.Atoi(m[1])
				conv, _ := strconv.Atoi(m[2])
				fail, _ := strconv.Atoi(m[3])
				if in != g.Stats.In {
					g.LastInChange = time.Now()
				}
				g.Stats, g.StatsAt = Stats{in, conv, fail}, time.Now()
			} else if errorRE.MatchString(line) {
				g.LastError, g.LastErrorAt = line, time.Now()
			}
		}
		s.mu.Unlock()
	}
	err := p.Wait()
	s.mu.Lock()
	defer s.mu.Unlock()
	if g.gen != gen { // stopped by Converge
		return
	}
	g.running = false
	g.LastExit = "exited"
	if err != nil {
		g.LastExit = err.Error()
	}
	g.LastError, g.LastErrorAt = "the pipe "+g.LastExit, time.Now()
	g.backoffUntil = time.Now().Add(backoff(g.Restarts))
}

// Run converges until ctx ends; expiry runs on the same loop.
func (s *Supervisor) Run(ctx context.Context) {
	t := time.NewTicker(s.interval)
	defer t.Stop()
	for {
		if _, err := s.reg.ExpireDue(ctx, time.Now()); err != nil {
			s.setErr(err)
		}
		if err := s.Converge(ctx); err != nil {
			s.setErr(err)
		} else {
			s.setErr(nil)
		}
		select {
		case <-ctx.Done():
			s.StopAll()
			return
		case <-t.C:
		}
	}
}

func (s *Supervisor) setErr(err error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if err == nil {
		s.LastConvergeEr = ""
	} else {
		s.LastConvergeEr = err.Error()
	}
}

func (s *Supervisor) StopAll() {
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, g := range s.groups {
		s.stopLocked(g)
	}
}

func (s *Supervisor) FeedView(feedID string) *GroupView {
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, g := range s.groups {
		for _, f := range g.Feeds {
			if f != feedID {
				continue
			}
			n := len(g.Outs)
			if n == 0 {
				n = 1
			}
			lines := g.Lines
			if len(lines) > 60 {
				lines = lines[len(lines)-60:]
			}
			v := &GroupView{Running: g.running, StartedAt: g.StartedAt, Restarts: g.Restarts, LastExit: g.LastExit,
				Outs: append([]string{}, g.Outs...), Feeds: append([]string{}, g.Feeds...), Command: g.Cmd,
				Stats: g.Stats, StatsAt: g.StatsAt, LastInChange: g.LastInChange, LastError: g.LastError,
				LastErrorAt: g.LastErrorAt, Lines: append([]string{}, lines...), SentEstimate: g.Stats.Converted / n,
				SharedWith: len(g.Feeds) - 1}
			if g.running && g.proc != nil {
				v.Pid = g.proc.Pid()
			}
			return v
		}
	}
	return nil
}

type SupervisorSummary struct {
	Groups       int       `json:"groups"`
	Running      int       `json:"running"`
	LastConverge time.Time `json:"last_converge_at"`
	LastError    string    `json:"last_converge_error"`
}

func (s *Supervisor) Summary() SupervisorSummary {
	s.mu.Lock()
	defer s.mu.Unlock()
	r := 0
	for _, g := range s.groups {
		if g.running {
			r++
		}
	}
	return SupervisorSummary{Groups: len(s.groups), Running: r, LastConverge: s.LastConverge, LastError: s.LastConvergeEr}
}

type metricRow struct {
	name   string
	labels map[string]string
	value  float64
}

func (s *Supervisor) Metrics() []metricRow {
	s.mu.Lock()
	defer s.mu.Unlock()
	var rows []metricRow
	for _, g := range s.groups {
		n := len(g.Outs)
		if n == 0 {
			n = 1
		}
		run := 0.0
		if g.running {
			run = 1
		}
		for _, f := range g.Feeds {
			l := map[string]string{"feed": f, "topic": g.Topic.ID, "format": g.Output.ToFormat}
			rows = append(rows,
				metricRow{"goat_feeds_feed_running", l, run},
				metricRow{"goat_feeds_feed_in_total", l, float64(g.Stats.In)},
				metricRow{"goat_feeds_feed_sent_total", l, float64(g.Stats.Converted / n)},
				metricRow{"goat_feeds_feed_failed_total", l, float64(g.Stats.Failed)},
				metricRow{"goat_feeds_feed_restarts_total", l, float64(g.Restarts)})
		}
	}
	return rows
}
