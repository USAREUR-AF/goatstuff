// SPDX-License-Identifier: Apache-2.0

package main

import (
	"context"
	"fmt"
	"net/http"
	"net/netip"
	"regexp"
	"strconv"
	"strings"
	"time"
)

const (
	RolePlatform = "platform-admin"
	RoleTactical = "tactical-admin"
	RoleMember   = "member"
)

var nameRE = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9 ._()/-]{0,99}$`)

// Refused is a request or action the policy refuses; Msg is shown to the person.
type Refused struct {
	Msg    string
	Status int
}

func (r *Refused) Error() string { return r.Msg }

func refuse(status int, format string, a ...any) *Refused {
	return &Refused{Msg: fmt.Sprintf(format, a...), Status: status}
}

type Viewer struct {
	Email   string   `json:"email"`
	Groups  []string `json:"groups"`
	Role    string   `json:"role"`
	IsAdmin bool     `json:"is_admin"`
}

// ViewerFromRequest reads identity from the sign-in fence. Only X-Forwarded-*: oauth2-proxy in
// reverse-proxy mode strips any client-supplied copy, and this service listens on loopback.
func ViewerFromRequest(r *http.Request, cfg *Config) *Viewer {
	email := strings.TrimSpace(r.Header.Get("X-Forwarded-Email"))
	if email == "" {
		return nil
	}
	var groups []string
	for _, g := range strings.Split(r.Header.Get("X-Forwarded-Groups"), ",") {
		if g = strings.TrimSpace(g); g != "" {
			groups = append(groups, g)
		}
	}
	v := &Viewer{Email: email, Groups: groups, Role: RoleMember}
	switch {
	case anyIn(groups, cfg.PlatformGroups):
		v.Role = RolePlatform
	case anyIn(groups, cfg.TacticalGroups):
		v.Role = RoleTactical
	}
	v.IsAdmin = v.Role != RoleMember
	return v
}

func anyIn(have, want []string) bool {
	for _, h := range have {
		for _, w := range want {
			if h == w {
				return true
			}
		}
	}
	return false
}

// Can says whether the viewer may take an action on a feed. Every rule the screens show is
// enforced here.
func Can(v *Viewer, action string, f Feed) bool {
	own := strings.EqualFold(f.Owner, v.Email)
	switch action {
	case "approve", "decline":
		return v.IsAdmin && f.Status == "pending"
	case "pause":
		return (v.IsAdmin || own) && f.Status == "active"
	case "resume":
		return (v.IsAdmin || own) && f.Status == "paused"
	case "remove":
		return (v.IsAdmin || own) && f.Status != "removed"
	case "renew":
		return v.IsAdmin && (f.Status == "active" || f.Status == "paused" || f.Status == "expired")
	case "check":
		return true
	}
	return false
}

var allActions = []string{"approve", "decline", "pause", "resume", "renew", "remove", "check"}

func ActionsFor(v *Viewer, f Feed) []string {
	out := []string{}
	for _, a := range allActions {
		if Can(v, a, f) {
			out = append(out, a)
		}
	}
	return out
}

// FeedRequest is the body of POST /api/feeds.
type FeedRequest struct {
	TopicID    string `json:"topic_id"`
	OutputID   string `json:"output_id"`
	DestName   string `json:"dest_name"`
	DestIP     string `json:"dest_ip"`
	DestPeerID string `json:"dest_peer_id"`
	Port       any    `json:"port"`
	Transport  string `json:"transport"`
	Days       any    `json:"days"`
	Purpose    string `json:"purpose"`
	// Receive side (direction "in"): FormatID names an intake format, Label names the
	// published feed, DestName/DestIP are the sending system, Port is on this core node.
	Direction string `json:"direction"`
	FormatID  string `json:"format_id"`
	Label     string `json:"label"`
	// Declare: the destination (or sending system) is not a declared system, so it is
	// declared with the request (tidy.go). An admin's is placed at once; a member's waits
	// in Tidy up.
	Declare *DeclareReq `json:"declare,omitempty"`
}

func toInt(v any) (int, bool) {
	switch t := v.(type) {
	case float64:
		return int(t), t == float64(int(t))
	case string:
		n, err := strconv.Atoi(strings.TrimSpace(t))
		return n, err == nil
	}
	return 0, false
}

// ValidateRequest checks a request and returns the feed to store with its first state.
func ValidateRequest(ctx context.Context, req FeedRequest, v *Viewer, cfg *Config, reg Registry, now time.Time) (Feed, error) {
	if req.Direction == "in" {
		return validateIntake(ctx, req, v, cfg, reg, now)
	}
	if req.Direction != "" && req.Direction != "out" {
		return Feed{}, refuse(400, "Direction is out (send a topic) or in (receive a system's data).")
	}
	topic := cfg.Topic(req.TopicID)
	if topic == nil {
		return Feed{}, refuse(400, "Choose a topic from the list.")
	}
	out := topic.Output(req.OutputID)
	if out == nil {
		return Feed{}, refuse(400, "Choose an output format offered for %s.", topic.Label)
	}
	name := strings.TrimSpace(req.DestName)
	if !nameRE.MatchString(name) {
		return Feed{}, refuse(400, "Give the receiving system a name (letters, digits, spaces, . _ - / ( )).")
	}
	ip, err := netip.ParseAddr(strings.TrimSpace(req.DestIP))
	if err != nil {
		return Feed{}, refuse(400, "The destination address is not an IP address.")
	}
	if !cfg.allowed.Contains(ip) {
		return Feed{}, refuse(400, "The destination must be a mesh address (%s); %s is not.", cfg.AllowedCIDR, ip)
	}
	port, ok := toInt(req.Port)
	if !ok {
		return Feed{}, refuse(400, "The port must be a number.")
	}
	if port < cfg.PortMin || port > cfg.PortMax {
		return Feed{}, refuse(400, "The port must be between %d and %d.", cfg.PortMin, cfg.PortMax)
	}
	transport := req.Transport
	if transport == "" {
		transport = "udp"
	}
	if transport != "udp" && transport != "tcp" {
		return Feed{}, refuse(400, "Transport is udp or tcp.")
	}
	used, err := reg.PortInUse(ctx, ip.String(), port, transport)
	if err != nil {
		return Feed{}, err
	}
	if used {
		return Feed{}, refuse(409, "%s:%d/%s already has a feed. One destination port takes one feed.", ip, port, transport)
	}
	days := cfg.DefaultDays
	if req.Days != nil && req.Days != "" {
		if days, ok = toInt(req.Days); !ok {
			return Feed{}, refuse(400, "The end date must be a number of days.")
		}
	}
	if days < 1 || days > cfg.MaxDays {
		return Feed{}, refuse(400, "A feed runs for 1 to %d days, then is renewed.", cfg.MaxDays)
	}
	purpose := strings.TrimSpace(req.Purpose)
	if len(purpose) < 3 {
		return Feed{}, refuse(400, "Say what the feed is for, so an approver can decide.")
	}
	if len(purpose) > 500 {
		return Feed{}, refuse(400, "Keep the purpose under 500 characters.")
	}
	f := Feed{TopicID: topic.ID, OutputID: out.ID, DestName: name, DestIP: ip.String(), DestPeerID: req.DestPeerID,
		Port: port, Transport: transport, Purpose: purpose, Owner: v.Email, EndAt: now.Add(time.Duration(days) * 24 * time.Hour),
		Direction: "out"}
	// The first state. Real data needs an admin; a synthetic topic may be self-service.
	switch {
	case v.IsAdmin:
		f.Status, f.DecidedBy, f.DecidedAt = "active", v.Email, &now
		f.DecisionNote = "switched on by its requester (" + v.Role + ")"
	case topic.DataClass == "synthetic" && cfg.SelfServiceSynthetic:
		f.Status, f.DecidedBy, f.DecidedAt = "active", v.Email, &now
		f.DecisionNote = "synthetic topic: self-service"
	default:
		f.Status = "pending"
	}
	return f, nil
}

func WaitsFor(cfg *Config) string {
	return "a tactical or platform admin (" + strings.Join(append(append([]string{}, cfg.TacticalGroups...), cfg.PlatformGroups...), ", ") + ")"
}

// validateIntake checks a request to receive a system's data on this core node.
func validateIntake(ctx context.Context, req FeedRequest, v *Viewer, cfg *Config, reg Registry, now time.Time) (Feed, error) {
	in := cfg.Intake
	if !in.Enabled() {
		return Feed{}, refuse(400, "This site does not receive data from systems yet: no receive ports are configured.")
	}
	format := in.Format(req.FormatID)
	if format == nil {
		return Feed{}, refuse(400, "Choose the standard the system sends.")
	}
	label := strings.TrimSpace(req.Label)
	if !nameRE.MatchString(label) {
		return Feed{}, refuse(400, "Name what the system sends, for example \"Blue C-UAS drone position\" (letters, digits, spaces, . _ - / ( )).")
	}
	name := strings.TrimSpace(req.DestName)
	if !nameRE.MatchString(name) {
		return Feed{}, refuse(400, "Give the sending system a name (letters, digits, spaces, . _ - / ( )).")
	}
	ip, err := netip.ParseAddr(strings.TrimSpace(req.DestIP))
	if err != nil {
		return Feed{}, refuse(400, "The sending system's address is not an IP address.")
	}
	if !cfg.allowed.Contains(ip) {
		return Feed{}, refuse(400, "The sending system must be on the mesh (%s); %s is not.", cfg.AllowedCIDR, ip)
	}
	port, ok := toInt(req.Port)
	if !ok || !in.HasPort(port) {
		return Feed{}, refuse(400, "Choose one of this site's receive ports: %v.", in.Ports)
	}
	transport := "udp"
	if req.Transport != "" && req.Transport != "udp" {
		return Feed{}, refuse(400, "Receiving is over UDP.")
	}
	used, err := reg.ListenPortInUse(ctx, port, transport)
	if err != nil {
		return Feed{}, err
	}
	if used {
		return Feed{}, refuse(409, "Receive port %d is already taken by another feed. Choose a free one.", port)
	}
	days := cfg.DefaultDays
	if req.Days != nil && req.Days != "" {
		if days, ok = toInt(req.Days); !ok {
			return Feed{}, refuse(400, "The end date must be a number of days.")
		}
	}
	if days < 1 || days > cfg.MaxDays {
		return Feed{}, refuse(400, "A feed runs for 1 to %d days, then is renewed.", cfg.MaxDays)
	}
	purpose := strings.TrimSpace(req.Purpose)
	if len(purpose) < 3 {
		return Feed{}, refuse(400, "Say what the data is and who uses it, so an approver can decide.")
	}
	if len(purpose) > 500 {
		return Feed{}, refuse(400, "Keep the purpose under 500 characters.")
	}
	f := Feed{TopicID: "intake", OutputID: format.ID, DestName: name, DestIP: ip.String(), DestPeerID: req.DestPeerID,
		Port: port, Transport: transport, Purpose: purpose, Owner: v.Email, EndAt: now.Add(time.Duration(days) * 24 * time.Hour),
		Direction: "in", Label: label}
	// Data from a system is real data: an admin switches it on; anyone else's request waits.
	if v.IsAdmin {
		f.Status, f.DecidedBy, f.DecidedAt = "active", v.Email, &now
		f.DecisionNote = "switched on by its requester (" + v.Role + ")"
	} else {
		f.Status = "pending"
	}
	return f, nil
}

// PublishedKey is where a received feed is published on the fabric.
func PublishedKey(cfg *Config, f Feed) string {
	return strings.TrimRight(cfg.Intake.KeyPrefix, "/") + "/" + f.ID
}
