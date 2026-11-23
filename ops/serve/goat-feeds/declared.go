// SPDX-License-Identifier: Apache-2.0
package main

// Declared links (ADR 1260): tactical links that deployment files start, not this
// service's form. Each role that starts links writes a manifest into DeclaredDir, one
// JSON file per declaration; this service lists them beside the requested feeds with
// the same two signals, read-only, naming the file and switch that control them.
//
// Until each link publishes its own health record on the fabric (ADR 1260, step 1),
// a background poller reads each link's systemd unit state and the converter's status
// line ("in=N converted=N failed=N") from the journal every 10 seconds. The poller runs
// on its own clock: page views read the last snapshot and never trigger a probe
// (ADR 1123).

import (
	"bufio"
	"context"
	"encoding/json"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"
)

// DeclaredLink is one entry of a manifest.
type DeclaredLink struct {
	ID           string `json:"id"`
	Unit         string `json:"unit"`
	Direction    string `json:"direction"`
	SourceKey    string `json:"source_key"`
	SourceLabel  string `json:"source_label"`
	ToFormat     string `json:"to_format"`
	OutputLabel  string `json:"output_label"`
	Dest         string `json:"dest"`
	DataClass    string `json:"data_class"`
	Owner        string `json:"owner"`
	ControlledBy string `json:"controlled_by"`
	Enabled      bool   `json:"enabled"`
}

type manifest struct {
	Owner string         `json:"owner"`
	Links []DeclaredLink `json:"links"`
}

// UnitProbe reads one unit's state and its recent status lines.
type UnitProbe interface {
	State(ctx context.Context, unit string) (active bool, since time.Time, err error)
	StatusLines(ctx context.Context, unit string, n int) ([]statusLine, error)
}

type statusLine struct {
	At    time.Time
	Stats Stats
}

var statusRE = regexp.MustCompile(`in=(\d+) converted=(\d+) failed=(\d+)`)

func parseStatus(msg string) (Stats, bool) {
	m := statusRE.FindStringSubmatch(msg)
	if m == nil {
		return Stats{}, false
	}
	in, _ := strconv.Atoi(m[1])
	conv, _ := strconv.Atoi(m[2])
	failed, _ := strconv.Atoi(m[3])
	return Stats{In: in, Converted: conv, Failed: failed}, true
}

// ExecProbe uses systemctl and journalctl (paths overridable for tests).
type ExecProbe struct{ Systemctl, Journalctl string }

func (p ExecProbe) State(ctx context.Context, unit string) (bool, time.Time, error) {
	out, err := exec.CommandContext(ctx, p.Systemctl, "show", unit, "-p", "ActiveState", "-p", "ActiveEnterTimestamp").Output()
	if err != nil {
		return false, time.Time{}, err
	}
	var active bool
	var since time.Time
	for _, l := range strings.Split(string(out), "\n") {
		k, v, _ := strings.Cut(strings.TrimSpace(l), "=")
		switch k {
		case "ActiveState":
			active = v == "active"
		case "ActiveEnterTimestamp":
			// "Mon 2026-11-23 08:40:02 UTC"
			if f := strings.Fields(v); len(f) >= 3 {
				since, _ = time.Parse("2006-01-02 15:04:05 MST", f[1]+" "+f[2]+" "+f[len(f)-1])
			}
		}
	}
	return active, since, nil
}

func (p ExecProbe) StatusLines(ctx context.Context, unit string, n int) ([]statusLine, error) {
	out, err := exec.CommandContext(ctx, p.Journalctl, "-u", unit, "-n", strconv.Itoa(n), "-o", "json", "--no-pager", "-q").Output()
	if err != nil {
		return nil, err
	}
	var lines []statusLine
	sc := bufio.NewScanner(strings.NewReader(string(out)))
	sc.Buffer(make([]byte, 64*1024), 1024*1024)
	for sc.Scan() {
		var e struct {
			Realtime string `json:"__REALTIME_TIMESTAMP"`
			Message  any    `json:"MESSAGE"`
		}
		if json.Unmarshal(sc.Bytes(), &e) != nil {
			continue
		}
		msg, _ := e.Message.(string)
		st, ok := parseStatus(msg)
		if !ok {
			continue
		}
		us, _ := strconv.ParseInt(e.Realtime, 10, 64)
		lines = append(lines, statusLine{At: time.UnixMicro(us), Stats: st})
	}
	return lines, nil
}

// declaredState is the poller's last reading of one link.
type declaredState struct {
	Link   DeclaredLink
	Source string // manifest file
	View   *GroupView
}

// Declared holds the manifests and the poller's snapshot.
type Declared struct {
	dir   string
	probe UnitProbe
	mu    sync.RWMutex
	snap  []declaredState
	// last time each link's input count moved, kept across polls
	lastMove map[string]time.Time
	lastIn   map[string]int
}

func NewDeclared(dir string, probe UnitProbe) *Declared {
	return &Declared{dir: dir, probe: probe, lastMove: map[string]time.Time{}, lastIn: map[string]int{}}
}

func (d *Declared) Enabled() bool { return d != nil && d.dir != "" }

// load reads every *.json manifest in the directory, sorted by file name.
func (d *Declared) load() []declaredState {
	files, _ := filepath.Glob(filepath.Join(d.dir, "*.json"))
	sort.Strings(files)
	var out []declaredState
	for _, f := range files {
		b, err := os.ReadFile(f)
		if err != nil {
			continue
		}
		var m manifest
		if json.Unmarshal(b, &m) != nil {
			continue
		}
		for _, l := range m.Links {
			if l.ID == "" || l.Unit == "" {
				continue
			}
			if l.Owner == "" {
				l.Owner = m.Owner
			}
			if l.Direction == "" {
				l.Direction = "out"
			}
			out = append(out, declaredState{Link: l, Source: filepath.Base(f)})
		}
	}
	return out
}

// Poll reads every declared link once.
func (d *Declared) Poll(ctx context.Context, now time.Time) {
	links := d.load()
	for i := range links {
		l := links[i].Link
		v := &GroupView{}
		cctx, cancel := context.WithTimeout(ctx, 3*time.Second)
		active, since, err := d.probe.State(cctx, l.Unit)
		if err != nil {
			v.LastError = "cannot read the unit: " + err.Error()
		}
		v.Running, v.StartedAt = active, since
		if !active && err == nil {
			v.LastError = l.Unit + " is not active"
		}
		if lines, err := d.probe.StatusLines(cctx, l.Unit, 12); err == nil && len(lines) > 0 {
			last := lines[len(lines)-1]
			v.Stats, v.StatsAt = last.Stats, last.At
			// the latest status line whose input count differs from the line before it
			for j := len(lines) - 1; j > 0; j-- {
				if lines[j].Stats.In != lines[j-1].Stats.In {
					if lines[j].At.After(d.lastMove[l.ID]) {
						d.lastMove[l.ID] = lines[j].At
					}
					break
				}
			}
			if prev, ok := d.lastIn[l.ID]; ok && prev != last.Stats.In && d.lastMove[l.ID].Before(last.At) {
				d.lastMove[l.ID] = last.At
			}
			d.lastIn[l.ID] = last.Stats.In
		} else if err != nil && v.LastError == "" {
			v.LastError = "cannot read its status lines: " + err.Error()
		}
		cancel()
		v.LastInChange = d.lastMove[l.ID]
		v.Command = []string{l.Unit}
		links[i].View = v
	}
	d.mu.Lock()
	d.snap = links
	d.mu.Unlock()
}

// Run polls every interval until ctx ends.
func (d *Declared) Run(ctx context.Context, every time.Duration) {
	if !d.Enabled() {
		return
	}
	d.Poll(ctx, time.Now())
	t := time.NewTicker(every)
	defer t.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case now := <-t.C:
			d.Poll(ctx, now)
		}
	}
}

func (d *Declared) Snapshot() []declaredState {
	if d == nil {
		return nil
	}
	d.mu.RLock()
	defer d.mu.RUnlock()
	return append([]declaredState(nil), d.snap...)
}

// splitDest turns "udp://198.51.100.20:7005" into ("udp", "198.51.100.20", 7005).
func splitDest(dest string) (string, string, int) {
	u, err := url.Parse(dest)
	if err != nil {
		return "", dest, 0
	}
	port, _ := strconv.Atoi(u.Port())
	return u.Scheme, u.Hostname(), port
}
