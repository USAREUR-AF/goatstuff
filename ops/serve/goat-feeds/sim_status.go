// SPDX-License-Identifier: Apache-2.0

package main

// Real or simulated: the out-of-band sim/real declaration for a received feed.
//
// Some sources (a drone autopilot sending MAVLink 2, for example) can run a simulation and send
// it in the same form as real data, with no flag a receiver can read. The source's own team
// declares, per received feed, what the source is doing: real, simulated or unknown, with a
// note. The declaration never changes what is received, published or sent: it is shown beside
// the data and served at GET /api/sim-status in the boundary node's document shape
// (goat.sim-status/v1, goatcore ops/simple-bn/sim_status.py), so a boundary node or any other
// system can read it and merge it with its own declarations.

import (
	"net/http"
	"strings"
	"time"
	"unicode/utf8"
)

const (
	SimSchema     = "goat.sim-status/v1"
	SimNoteMax    = 300
	simActionName = "sim-status"
)

// SimModes are the declarations a person may make, the same three the boundary node uses.
var SimModes = []string{"real", "simulated", "unknown"}

// SimStatus is one feed's declaration.
type SimStatus struct {
	Mode  string    `json:"mode"`
	SetAt time.Time `json:"set_at"`
	SetBy string    `json:"set_by"`
	Note  string    `json:"note"`
}

// ValidateSim checks a declaration's mode and note and returns them cleaned.
func ValidateSim(mode, note string) (string, string, error) {
	mode = strings.ToLower(strings.TrimSpace(mode))
	ok := false
	for _, m := range SimModes {
		ok = ok || m == mode
	}
	if !ok {
		return "", "", refuse(400, "Choose real, simulated or unknown.")
	}
	note = strings.Join(strings.Fields(note), " ")
	if utf8.RuneCountInString(note) > SimNoteMax {
		return "", "", refuse(400, "Keep the note under %d characters.", SimNoteMax)
	}
	return mode, note, nil
}

// simEventDetail is the audit log line for a declaration.
func simEventDetail(mode, note string) string {
	if note == "" {
		return mode
	}
	return mode + ": " + note
}

// simFeedDoc is one feed in GET /api/sim-status. set_at is Unix seconds (float), as the
// boundary node's own file writes it, so a reader can compare and merge the two.
type simFeedDoc struct {
	Mode  string  `json:"mode"`
	SetAt float64 `json:"set_at"`
	SetBy string  `json:"set_by"`
	Note  string  `json:"note"`
	Label string  `json:"label"`
}

type simDoc struct {
	Schema string                `json:"schema"`
	Feeds  map[string]simFeedDoc `json:"feeds"`
}

// simStatusDoc is the machine-readable document: every received feed that has a declaration
// and is not removed or declined, keyed by feed id.
func simStatusDoc(feeds []Feed) simDoc {
	doc := simDoc{Schema: SimSchema, Feeds: map[string]simFeedDoc{}}
	for _, f := range feeds {
		if f.Sim == nil || f.Direction != "in" || f.Status == "removed" || f.Status == "declined" {
			continue
		}
		doc.Feeds[f.ID] = simFeedDoc{Mode: f.Sim.Mode, SetAt: float64(f.Sim.SetAt.UnixNano()) / 1e9,
			SetBy: f.Sim.SetBy, Note: f.Sim.Note, Label: f.Label}
	}
	return doc
}

// simStatus serves GET /api/sim-status. It is read-only and needs no sign-in (the sign-in
// fence lets this one path through): it holds no secret, and the service is reachable only on
// the mesh, the same posture as the boundary node's own status API.
func (s *Server) simStatus(w http.ResponseWriter, r *http.Request) {
	feeds, err := s.reg.List(r.Context())
	if err != nil {
		s.fail(w, err)
		return
	}
	writeJSON(w, 200, simStatusDoc(feeds))
}

// publishedSims maps each published topic id ("in-<feed id>") to its feed's declaration.
func publishedSims(feeds []Feed) map[string]*SimStatus {
	out := map[string]*SimStatus{}
	for _, f := range feeds {
		if f.Direction == "in" && f.Sim != nil {
			out["in-"+f.ID] = f.Sim
		}
	}
	return out
}

// withSims returns the topics with each published topic's declaration attached, in catalog order.
func withSims(topics []Topic, sims map[string]*SimStatus) []Topic {
	out := make([]Topic, len(topics))
	copy(out, topics)
	for i := range out {
		if sm, ok := sims[out[i].ID]; ok {
			out[i].SimStatus = sm
		}
	}
	return out
}
