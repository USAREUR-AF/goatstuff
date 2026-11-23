// SPDX-License-Identifier: Apache-2.0
package main

import (
	"fmt"
	"time"
)

// Signal is one yes/no fact about a feed with the reason behind it. A feed carries two,
// shown side by side and never folded into one word:
//
//   - Integrated: the link is set up and its process is running, so data would go
//     through if there were any.
//   - Flowing:    data actually went through in the last minute (sending for an OUT
//     feed, receiving for an IN feed).
//
// An integrated feed whose source is idle (a radar switched off) reads Integrated yes,
// Flowing no, with the reason "nothing from the topic in the last minute".
type Signal struct {
	On   bool   `json:"on"`
	Word string `json:"word"`
	Note string `json:"note"`
}

// flowWindow is how recent the last message must be for a feed to count as flowing.
const flowWindow = 60 * time.Second

func yes(note string) Signal { return Signal{On: true, Word: "Yes", Note: note} }
func no(note string) Signal  { return Signal{On: false, Word: "No", Note: note} }

// ago renders a duration for a reason line: "4 s", "12 min", "3 h".
func agoText(d time.Duration) string {
	switch {
	case d < time.Minute:
		return fmt.Sprintf("%d s", int(d.Seconds()))
	case d < time.Hour:
		return fmt.Sprintf("%d min", int(d.Minutes()))
	case d < 48*time.Hour:
		return fmt.Sprintf("%d h", int(d.Hours()))
	}
	return fmt.Sprintf("%d d", int(d.Hours()/24))
}

// linkSignals answers the two questions for one feed. in is true for a receive feed;
// gone is true when its topic (OUT) or standard (IN) is no longer offered at the site.
func linkSignals(f Feed, d *GroupView, gone, in bool, now time.Time) (integrated, flowing Signal) {
	source, verb := "the topic", "sent"
	if in {
		source, verb = "the sender", "received"
	}
	switch f.Status {
	case "pending":
		integrated = no("Waiting for approval.")
	case "paused":
		integrated = no("Paused.")
	case "declined", "expired", "removed":
		integrated = no("Feed " + f.Status + ".")
	default:
		switch {
		case gone && in:
			integrated = no("Its standard is no longer offered at this site.")
		case gone:
			integrated = no("The topic is no longer in this site's catalog.")
		case d == nil:
			integrated = no("Starting (a few seconds).")
		case !d.Running:
			integrated = no("The process is not running: " + d.LastError)
		case d.StartedAt.IsZero():
			integrated = yes("Process running.")
		default:
			integrated = yes("Process running since " + d.StartedAt.UTC().Format("02 Jan 15:04Z") + ".")
		}
	}
	if !integrated.On {
		return integrated, no("Not integrated.")
	}
	switch {
	case d.StatsAt.IsZero() || d.Stats.In == 0:
		flowing = no("Nothing from " + source + " since the process started.")
	case d.Stats.Failed > 0 && d.Stats.Converted == 0:
		flowing = no("Data arrives but none converts: " + d.LastError)
	case now.Sub(d.LastInChange) > flowWindow:
		flowing = no("Nothing from " + source + " in the last minute; last data " + agoText(now.Sub(d.LastInChange)) + " ago.")
	default:
		flowing = yes(fmt.Sprintf("Last data %s ago; %d %s since start.", agoText(now.Sub(d.LastInChange)), d.Stats.Converted, verb))
	}
	return integrated, flowing
}
