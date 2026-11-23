// SPDX-License-Identifier: Apache-2.0
package main

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

type fakeProbe struct {
	active map[string]bool
	lines  map[string][]statusLine
}

func (f fakeProbe) State(_ context.Context, unit string) (bool, time.Time, error) {
	return f.active[unit], time.Now().Add(-time.Hour), nil
}
func (f fakeProbe) StatusLines(_ context.Context, unit string, n int) ([]statusLine, error) {
	return f.lines[unit], nil
}

func writeManifest(t *testing.T, dir, name, body string) {
	t.Helper()
	if err := os.WriteFile(filepath.Join(dir, name), []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
}

func TestDeclaredRows(t *testing.T) {
	dir := t.TempDir()
	writeManifest(t, dir, "pipeline.json", `{"owner":"processors pipeline test","links":[
	 {"id":"fused-a","unit":"goat-xf-pipe@fused.service","source_label":"Fused air picture","output_label":"ASTERIX CAT062","dest":"udp://198.51.100.20:7005","enabled":true,"controlled_by":"destinations.x.enabled"},
	 {"id":"raw-idle","unit":"goat-xf-pipe@raw.service","source_label":"Radar (raw)","dest":"udp://198.51.100.21:7005","enabled":true},
	 {"id":"stopped","unit":"goat-xf-pipe@gone.service","source_label":"Gone","dest":"udp://198.51.100.22:7005","enabled":true},
	 {"id":"off","unit":"goat-xf-pipe@off.service","source_label":"Off","dest":"udp://198.51.100.23:7005","enabled":false}]}`)
	writeManifest(t, dir, "broken.json", `not json`)
	now := time.Now()
	probe := fakeProbe{
		active: map[string]bool{"goat-xf-pipe@fused.service": true, "goat-xf-pipe@raw.service": true},
		lines: map[string][]statusLine{
			"goat-xf-pipe@fused.service": {{now.Add(-20 * time.Second), Stats{In: 100, Converted: 300}}, {now.Add(-10 * time.Second), Stats{In: 120, Converted: 360}}},
			"goat-xf-pipe@raw.service":   {{now.Add(-20 * time.Second), Stats{In: 7, Converted: 7}}, {now.Add(-10 * time.Second), Stats{In: 7, Converted: 7}}},
		},
	}
	d := NewDeclared(dir, probe)
	d.Poll(context.Background(), now)
	s := &Server{cfg: &Config{}, decl: d}
	rows := s.declaredRows(context.Background(), now)
	if len(rows) != 4 {
		t.Fatalf("rows: %d, want 4 (the unreadable manifest is skipped)", len(rows))
	}
	want := map[string][2]bool{"declared:fused-a": {true, true}, "declared:raw-idle": {true, false},
		"declared:stopped": {false, false}, "declared:off": {false, false}}
	for _, r := range rows {
		w := want[r.ID]
		if r.Integrated.On != w[0] || r.Flowing.On != w[1] {
			t.Errorf("%s: integrated=%v flowing=%v (%s / %s), want %v %v", r.ID, r.Integrated.On, r.Flowing.On, r.Integrated.Note, r.Flowing.Note, w[0], w[1])
		}
		if r.Kind != "declared" || r.Manifest != "pipeline.json" || r.Owner != "processors pipeline test" {
			t.Errorf("%s: kind %q manifest %q owner %q", r.ID, r.Kind, r.Manifest, r.Owner)
		}
	}
	if rows[0].DestIP != "198.51.100.20" || rows[0].Port != 7005 || rows[0].Transport != "udp" || rows[0].ControlledBy == "" {
		t.Errorf("fused row: %+v", rows[0].Feed)
	}
	if !strings.Contains(rows[3].Integrated.Note, "Switched off") {
		t.Errorf("off row: %q", rows[3].Integrated.Note)
	}
}

func TestParseStatusAndState(t *testing.T) {
	st, ok := parseStatus("goat-xf-pipe-zenoh: in=10540 converted=31620 failed=2 450s")
	if !ok || st.In != 10540 || st.Converted != 31620 || st.Failed != 2 {
		t.Errorf("parse: %+v %v", st, ok)
	}
	if _, ok := parseStatus("starting"); ok {
		t.Error("a non-status line parsed")
	}
	if tr, ip, port := splitDest("udp://198.51.100.9:40153"); tr != "udp" || ip != "198.51.100.9" || port != 40153 {
		t.Errorf("splitDest: %s %s %d", tr, ip, port)
	}
}
