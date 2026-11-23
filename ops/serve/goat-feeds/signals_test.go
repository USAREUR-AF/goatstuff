// SPDX-License-Identifier: Apache-2.0
package main

import (
	"strings"
	"testing"
	"time"
)

func TestLinkSignals(t *testing.T) {
	now := time.Now()
	run := func(in, conv, failed int, last time.Duration) *GroupView {
		return &GroupView{Running: true, StartedAt: now.Add(-time.Hour), StatsAt: now, LastInChange: now.Add(-last),
			Stats: Stats{In: in, Converted: conv, Failed: failed}, LastError: "no kind"}
	}
	cases := []struct {
		name           string
		f              Feed
		d              *GroupView
		gone, in       bool
		integ, flowing bool
		note           string // must appear in the flowing (or, when not integrated, integrated) reason
	}{
		{"flowing out", Feed{Status: "active"}, run(10, 10, 0, 2*time.Second), false, false, true, true, "since start"},
		{"source idle: integrated, not flowing", Feed{Status: "active"}, run(10, 10, 0, 3*time.Hour), false, false, true, false, "last data 3 h ago"},
		{"never any data", Feed{Status: "active"}, run(0, 0, 0, 0), false, false, true, false, "since the process started"},
		{"nothing converts", Feed{Status: "active"}, run(5, 0, 5, time.Second), false, false, true, false, "none converts"},
		{"receive flowing", Feed{Status: "active", Direction: "in"}, run(9, 9, 0, time.Second), false, true, true, true, "received"},
		{"receive, sender idle", Feed{Status: "active", Direction: "in"}, run(9, 9, 0, 10*time.Minute), false, true, true, false, "the sender"},
		{"waiting", Feed{Status: "pending"}, nil, false, false, false, false, "approval"},
		{"paused", Feed{Status: "paused"}, nil, false, false, false, false, "Paused"},
		{"process stopped", Feed{Status: "active"}, &GroupView{Running: false, LastError: "exited 1"}, false, false, false, false, "exited 1"},
		{"starting", Feed{Status: "active"}, nil, false, false, false, false, "Starting"},
		{"topic gone", Feed{Status: "active"}, run(1, 1, 0, 0), true, false, false, false, "catalog"},
	}
	for _, c := range cases {
		i, fl := linkSignals(c.f, c.d, c.gone, c.in, now)
		if i.On != c.integ || fl.On != c.flowing {
			t.Errorf("%s: integrated=%v flowing=%v, want %v %v", c.name, i.On, fl.On, c.integ, c.flowing)
		}
		reason := fl.Note
		if !i.On {
			reason = i.Note
		}
		if !strings.Contains(reason, c.note) {
			t.Errorf("%s: reason %q lacks %q", c.name, reason, c.note)
		}
		if (i.On && i.Word != "Yes") || (!fl.On && fl.Word != "No") {
			t.Errorf("%s: words %q %q", c.name, i.Word, fl.Word)
		}
	}
}
