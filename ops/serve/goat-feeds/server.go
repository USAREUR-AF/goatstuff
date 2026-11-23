// SPDX-License-Identifier: Apache-2.0

package main

import (
	"context"
	"embed"
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"net"
	"net/http"
	"os/exec"
	"sort"
	"strings"
	"time"
)

//go:embed web
var webFS embed.FS

type Server struct {
	cfg   *Config
	reg   Registry
	sup   *Supervisor
	peers *Peers
	decl  *Declared
	start time.Time
}

func (s *Server) Routes() http.Handler {
	mux := http.NewServeMux()
	static, _ := fs.Sub(webFS, "web")
	mux.Handle("GET /", http.FileServer(http.FS(static)))
	mux.HandleFunc("GET /healthz", s.healthz)
	mux.HandleFunc("GET /metrics", s.metrics)
	mux.HandleFunc("GET /api/me", s.auth(s.me))
	mux.HandleFunc("GET /api/catalog", s.auth(s.catalog))
	mux.HandleFunc("GET /api/intake", s.auth(s.intake))
	mux.HandleFunc("GET /api/peers", s.auth(s.peersSearch))
	mux.HandleFunc("GET /api/feeds", s.auth(s.listFeeds))
	mux.HandleFunc("POST /api/feeds", s.auth(s.createFeed))
	mux.HandleFunc("GET /api/feeds/{id}", s.auth(s.getFeed))
	mux.HandleFunc("POST /api/feeds/{id}/{action}", s.auth(s.feedAction))
	mux.HandleFunc("GET /api/audit", s.auth(s.audit))
	return mux
}

type authed func(w http.ResponseWriter, r *http.Request, v *Viewer)

func (s *Server) auth(h authed) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		v := ViewerFromRequest(r, s.cfg)
		if v == nil {
			writeErr(w, 401, "Sign in first.")
			return
		}
		// A cross-site form cannot set this header, so POSTs carrying it came from this page.
		if r.Method == "POST" && r.Header.Get("X-Goat-Feeds") != "1" {
			writeErr(w, 403, "Requests must come from the goat-feeds page.")
			return
		}
		h(w, r, v)
	}
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}

func writeErr(w http.ResponseWriter, status int, msg string) {
	writeJSON(w, status, map[string]string{"error": msg})
}

func (s *Server) fail(w http.ResponseWriter, err error) {
	var ref *Refused
	if errors.As(err, &ref) {
		writeErr(w, ref.Status, ref.Msg)
		return
	}
	if errors.Is(err, ErrNotFound) {
		writeErr(w, 404, "No such feed.")
		return
	}
	writeErr(w, 500, "Internal error: "+err.Error())
}

func (s *Server) healthz(w http.ResponseWriter, r *http.Request) {
	ctx, cancel := context.WithTimeout(r.Context(), 3*time.Second)
	defer cancel()
	sum := s.sup.Summary()
	body := map[string]any{"status": "up", "site": s.cfg.Site, "uptime_s": int(time.Since(s.start).Seconds()),
		"supervisor": sum, "reasons": []string{}}
	reasons := []string{}
	if err := s.reg.Ping(ctx); err != nil {
		reasons = append(reasons, "database unreachable: "+err.Error())
	}
	if sum.LastError != "" {
		reasons = append(reasons, "delivery loop: "+sum.LastError)
	}
	if !sum.LastConverge.IsZero() && time.Since(sum.LastConverge) > 30*time.Second {
		reasons = append(reasons, "delivery loop has not run for "+time.Since(sum.LastConverge).Round(time.Second).String())
	}
	if sum.Groups > sum.Running {
		reasons = append(reasons, fmt.Sprintf("%d of %d delivery processes not running", sum.Groups-sum.Running, sum.Groups))
	}
	status := 200
	if len(reasons) > 0 {
		body["status"] = "degraded"
		status = 503
	}
	body["reasons"] = reasons
	writeJSON(w, status, body)
}

func (s *Server) metrics(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "text/plain; version=0.0.4")
	feeds, err := s.reg.List(r.Context())
	byStatus := map[string]int{}
	if err == nil {
		for _, f := range feeds {
			byStatus[f.Status]++
		}
	}
	fmt.Fprintln(w, "# HELP goat_feeds_feeds Feeds by state.")
	fmt.Fprintln(w, "# TYPE goat_feeds_feeds gauge")
	for _, st := range []string{"pending", "active", "paused", "declined", "expired", "removed"} {
		fmt.Fprintf(w, "goat_feeds_feeds{state=%q} %d\n", st, byStatus[st])
	}
	sum := s.sup.Summary()
	fmt.Fprintf(w, "# TYPE goat_feeds_groups gauge\ngoat_feeds_groups %d\n", sum.Groups)
	fmt.Fprintf(w, "# TYPE goat_feeds_groups_running gauge\ngoat_feeds_groups_running %d\n", sum.Running)
	db := 1
	if err != nil {
		db = 0
	}
	fmt.Fprintf(w, "# TYPE goat_feeds_db_up gauge\ngoat_feeds_db_up %d\n", db)
	rows := s.sup.Metrics()
	sort.Slice(rows, func(i, j int) bool { return rows[i].name < rows[j].name })
	last := ""
	for _, m := range rows {
		if m.name != last {
			kind := "counter"
			if m.name == "goat_feeds_feed_running" {
				kind = "gauge"
			}
			fmt.Fprintf(w, "# TYPE %s %s\n", m.name, kind)
			last = m.name
		}
		fmt.Fprintf(w, "%s{feed=%q,topic=%q,format=%q} %g\n", m.name, m.labels["feed"], m.labels["topic"], m.labels["format"], m.value)
	}
}

func (s *Server) me(w http.ResponseWriter, r *http.Request, v *Viewer) {
	writeJSON(w, 200, map[string]any{"viewer": v, "site": s.cfg.Site, "goatnet": s.cfg.Goatnet,
		"waits_for": WaitsFor(s.cfg), "self_service_synthetic": s.cfg.SelfServiceSynthetic,
		"default_days": s.cfg.DefaultDays, "max_days": s.cfg.MaxDays, "port_min": s.cfg.PortMin, "port_max": s.cfg.PortMax,
		"peers_enabled": s.peers.Enabled(), "peers_error": s.peers.LastErr})
}

func (s *Server) catalog(w http.ResponseWriter, r *http.Request, v *Viewer) {
	writeJSON(w, 200, map[string]any{"topics": s.cfg.AllTopics()})
}

// intake describes the receive side: where systems send, which ports are free, which standards.
func (s *Server) intake(w http.ResponseWriter, r *http.Request, v *Viewer) {
	in := s.cfg.Intake
	if !in.Enabled() {
		writeJSON(w, 200, map[string]any{"enabled": false})
		return
	}
	ports := []map[string]any{}
	for _, p := range in.Ports {
		used, err := s.reg.ListenPortInUse(r.Context(), p, "udp")
		if err != nil {
			s.fail(w, err)
			return
		}
		ports = append(ports, map[string]any{"port": p, "free": !used})
	}
	writeJSON(w, 200, map[string]any{"enabled": true, "listen_ip": in.ListenIP, "ports": ports,
		"formats": in.Formats, "outputs": in.Outputs, "publishes_as": in.ToFormat})
}

func (s *Server) peersSearch(w http.ResponseWriter, r *http.Request, v *Viewer) {
	writeJSON(w, 200, map[string]any{"enabled": s.peers.Enabled(), "error": s.peers.LastErr,
		"peers": s.peers.Search(r.Context(), r.URL.Query().Get("q"), 25)})
}

type feedOut struct {
	Feed
	TopicLabel  string     `json:"topic_label"`
	OutputLabel string     `json:"output_label"`
	ToFormat    string     `json:"to_format"`
	DataClass   string     `json:"data_class"`
	Kind        string     `json:"kind"`
	Actions     []string   `json:"actions"`
	Delivery    *GroupView `json:"delivery"`
	Health      string     `json:"health"`
	HealthNote  string     `json:"health_note"`
	// Two separate facts, never folded together (signals.go).
	Integrated Signal `json:"integrated"`
	Flowing    Signal `json:"flowing"`
	// Receive side only.
	ListenAt     string `json:"listen_at,omitempty"`
	PublishedKey string `json:"published_key,omitempty"`
	PublishedAs  string `json:"published_as,omitempty"`
	TopicID      string `json:"published_topic_id,omitempty"`
	ReadersCount int    `json:"readers,omitempty"`
	// Declared links only (ADR 1260): started by a deployment file, shown read-only.
	ControlledBy string `json:"controlled_by,omitempty"`
	// The sensors or systems a declared link's data comes from (flow view).
	Sources  []string `json:"sources,omitempty"`
	Manifest string   `json:"manifest,omitempty"`
}

func (s *Server) decorate(f Feed, v *Viewer, detail bool) feedOut {
	o := feedOut{Feed: f, Actions: ActionsFor(v, f)}
	if f.Direction == "in" {
		o.TopicLabel, o.DataClass, o.Kind = f.Label, "real", "received"
		if s.cfg.Intake.Enabled() {
			if ft := s.cfg.Intake.Format(f.OutputID); ft != nil {
				o.OutputLabel, o.ToFormat = ft.Label, ft.FromFormat
			}
			o.ListenAt = fmt.Sprintf("%s:%d/udp", s.cfg.Intake.ListenIP, f.Port)
			o.PublishedKey, o.PublishedAs, o.TopicID = PublishedKey(s.cfg, f), s.cfg.Intake.ToFormat, "in-"+f.ID
		}
		d := s.sup.FeedView(f.ID)
		o.Health, o.HealthNote = intakeHealth(f, d, o.OutputLabel == "")
		o.Integrated, o.Flowing = linkSignals(f, d, o.OutputLabel == "", true, time.Now())
		if !detail && d != nil {
			d.Lines, d.Command = nil, nil
		}
		o.Delivery = d
		return o
	}
	if t := s.cfg.Topic(f.TopicID); t != nil {
		o.TopicLabel, o.DataClass, o.Kind = t.Label, t.DataClass, t.Kind
		if out := t.Output(f.OutputID); out != nil {
			o.OutputLabel, o.ToFormat = out.Label, out.ToFormat
		}
	}
	d := s.sup.FeedView(f.ID)
	o.Health, o.HealthNote = feedHealth(f, d, o.TopicLabel == "")
	o.Integrated, o.Flowing = linkSignals(f, d, o.TopicLabel == "", false, time.Now())
	if detail {
		o.Delivery = d
	} else if d != nil {
		d.Lines, d.Command = nil, nil
		o.Delivery = d
	}
	return o
}

// feedHealth turns a feed's state and its delivery process into one word and one sentence.
func feedHealth(f Feed, d *GroupView, topicGone bool) (string, string) {
	switch f.Status {
	case "pending":
		return "waiting", "Waiting for approval."
	case "paused":
		return "paused", "Paused; nothing is sent."
	case "declined", "expired", "removed":
		return f.Status, ""
	}
	if topicGone {
		return "broken", "The topic is no longer in this site's catalog."
	}
	if d == nil {
		return "starting", "The delivery loop has not picked it up yet (a few seconds)."
	}
	if !d.Running {
		return "broken", "The delivery process is not running: " + d.LastError
	}
	if d.StatsAt.IsZero() {
		return "starting", "Started; no counters yet."
	}
	if d.Stats.Failed > 0 && d.Stats.Converted == 0 {
		return "broken", "Messages arrive but none convert. Last error: " + d.LastError
	}
	if d.Stats.In == 0 || time.Since(d.LastInChange) > 60*time.Second {
		return "quiet", "Running, but no message has arrived from the topic in the last minute."
	}
	if f.Transport == "udp" {
		return "sending", "Sending over UDP. UDP gives no answer back, so receipt cannot be seen from here."
	}
	return "sending", "Sending over TCP."
}

// intakeHealth is feedHealth for the receive side.
func intakeHealth(f Feed, d *GroupView, formatGone bool) (string, string) {
	switch f.Status {
	case "pending":
		return "waiting", "Waiting for approval."
	case "paused":
		return "paused", "Paused; nothing is received or published."
	case "declined", "expired", "removed":
		return f.Status, ""
	}
	if formatGone {
		return "broken", "Its standard is no longer offered at this site."
	}
	if d == nil {
		return "starting", "The loop has not picked it up yet (a few seconds)."
	}
	if !d.Running {
		return "broken", "The receiving process is not running: " + d.LastError
	}
	if d.StatsAt.IsZero() || d.Stats.In == 0 {
		return "listening", "Listening; nothing has arrived from the sender yet. Check the sender is sending to the address shown."
	}
	if d.Stats.Failed > 0 && d.Stats.Converted == 0 {
		return "broken", "Data arrives but none of it reads as the chosen standard. Last error: " + d.LastError
	}
	if time.Since(d.LastInChange) > 60*time.Second {
		return "quiet", "Data arrived earlier, but nothing in the last minute."
	}
	return "receiving", "Receiving and publishing on the fabric."
}

func (s *Server) listFeeds(w http.ResponseWriter, r *http.Request, v *Viewer) {
	feeds, err := s.reg.List(r.Context())
	if err != nil {
		s.fail(w, err)
		return
	}
	out := make([]feedOut, 0, len(feeds))
	for _, f := range feeds {
		out = append(out, s.decorate(f, v, false))
	}
	out = append(out, s.declaredRows(r.Context(), time.Now())...)
	writeJSON(w, 200, map[string]any{"feeds": out})
}

func (s *Server) createFeed(w http.ResponseWriter, r *http.Request, v *Viewer) {
	var req FeedRequest
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 64*1024)).Decode(&req); err != nil {
		writeErr(w, 400, "The request is not valid JSON.")
		return
	}
	if req.DestPeerID == "" && s.peers.Enabled() {
		if p := s.peers.ByIP(r.Context(), strings.TrimSpace(req.DestIP)); p != nil {
			req.DestPeerID = p.ID
		}
	}
	f, err := ValidateRequest(r.Context(), req, v, s.cfg, s.reg, time.Now())
	if err != nil {
		s.fail(w, err)
		return
	}
	detail := fmt.Sprintf("%s → %s %s:%d/%s: %s", f.TopicID, f.DestName, f.DestIP, f.Port, f.Transport, f.Purpose)
	if f.Direction == "in" {
		detail = fmt.Sprintf("receive %q (%s) from %s %s on port %d/%s: %s", f.Label, f.OutputID, f.DestName, f.DestIP, f.Port, f.Transport, f.Purpose)
	}
	created, err := s.reg.Create(r.Context(), f, v.Email, detail)
	if err != nil {
		s.fail(w, err)
		return
	}
	writeJSON(w, 201, s.decorate(created, v, true))
}

func (s *Server) getFeed(w http.ResponseWriter, r *http.Request, v *Viewer) {
	f, err := s.reg.Get(r.Context(), r.PathValue("id"))
	if err != nil {
		s.fail(w, err)
		return
	}
	ev, err := s.reg.Events(r.Context(), f.ID, 200)
	if err != nil {
		s.fail(w, err)
		return
	}
	writeJSON(w, 200, map[string]any{"feed": s.decorate(f, v, true), "events": ev})
}

type actionBody struct {
	Note string `json:"note"`
	Days int    `json:"days"`
}

func (s *Server) feedAction(w http.ResponseWriter, r *http.Request, v *Viewer) {
	ctx := r.Context()
	f, err := s.reg.Get(ctx, r.PathValue("id"))
	if err != nil {
		s.fail(w, err)
		return
	}
	action := r.PathValue("action")
	var body actionBody
	_ = json.NewDecoder(http.MaxBytesReader(w, r.Body, 16*1024)).Decode(&body)
	note := strings.TrimSpace(body.Note)
	if len(note) > 500 {
		note = note[:500]
	}
	if !Can(v, action, f) {
		writeErr(w, 403, fmt.Sprintf("You cannot %s this feed (it is %s, and you are a %s).", action, f.Status, v.Role))
		return
	}
	var out Feed
	switch action {
	case "approve":
		out, err = s.reg.Transition(ctx, f.ID, "active", v.Email, "approve", note, true, nil)
	case "decline":
		if note == "" {
			writeErr(w, 400, "Say why it is declined, so the requester knows what to change.")
			return
		}
		out, err = s.reg.Transition(ctx, f.ID, "declined", v.Email, "decline", note, true, nil)
	case "pause":
		out, err = s.reg.Transition(ctx, f.ID, "paused", v.Email, "pause", note, false, nil)
	case "resume":
		out, err = s.reg.Transition(ctx, f.ID, "active", v.Email, "resume", note, false, nil)
	case "remove":
		out, err = s.reg.Transition(ctx, f.ID, "removed", v.Email, "remove", note, false, nil)
	case "renew":
		days := body.Days
		if days <= 0 {
			days = s.cfg.DefaultDays
		}
		if days > s.cfg.MaxDays {
			writeErr(w, 400, fmt.Sprintf("A feed runs for at most %d days.", s.cfg.MaxDays))
			return
		}
		end := time.Now().Add(time.Duration(days) * 24 * time.Hour)
		status := f.Status
		if status == "expired" {
			status = "active"
		}
		out, err = s.reg.Transition(ctx, f.ID, status, v.Email, "renew", fmt.Sprintf("%d days. %s", days, note), false, &end)
	case "check":
		res := checkDestination(ctx, f)
		if f.Direction == "in" {
			delete(res, "udp_note")
			res["receive_note"] = "The sender must send to this core node over UDP; the counters above show whether anything arrives. A ping only shows the sender is on the mesh."
		}
		writeJSON(w, 200, res)
		return
	default:
		writeErr(w, 404, "Unknown action.")
		return
	}
	if err != nil {
		s.fail(w, err)
		return
	}
	writeJSON(w, 200, s.decorate(out, v, true))
}

// checkDestination answers "can the core node reach the destination?": ping, and a TCP connect
// for TCP feeds. A UDP port cannot be probed without an answer from the receiver.
func checkDestination(ctx context.Context, f Feed) map[string]any {
	res := map[string]any{"at": time.Now()}
	cctx, cancel := context.WithTimeout(ctx, 4*time.Second)
	defer cancel()
	out, err := exec.CommandContext(cctx, "ping", "-c", "1", "-W", "2", f.DestIP).CombinedOutput()
	res["ping_ok"] = err == nil
	if err != nil {
		res["ping_detail"] = lastLine(string(out))
	}
	if f.Transport == "tcp" {
		c, err := net.DialTimeout("tcp", fmt.Sprintf("%s:%d", f.DestIP, f.Port), 3*time.Second)
		res["tcp_ok"] = err == nil
		if err != nil {
			res["tcp_detail"] = err.Error()
		} else {
			_ = c.Close()
		}
	} else {
		res["udp_note"] = "UDP cannot be probed from here: a receiver that drops or accepts datagrams looks the same. Ask the receiving system's owner whether data arrives."
	}
	return res
}

func lastLine(s string) string {
	s = strings.TrimSpace(s)
	if i := strings.LastIndex(s, "\n"); i >= 0 {
		return s[i+1:]
	}
	return s
}

func (s *Server) audit(w http.ResponseWriter, r *http.Request, v *Viewer) {
	ev, err := s.reg.Events(r.Context(), "", 500)
	if err != nil {
		s.fail(w, err)
		return
	}
	writeJSON(w, 200, map[string]any{"events": ev})
}

// declaredRows renders the poller's snapshot as read-only feed rows.
func (s *Server) declaredRows(ctx context.Context, now time.Time) []feedOut {
	var out []feedOut
	for _, ds := range s.decl.Snapshot() {
		l := ds.Link
		tr, ip, port := splitDest(l.Dest)
		f := Feed{ID: "declared:" + l.ID, Direction: l.Direction, Status: "active", DestIP: ip, Port: port,
			Transport: tr, Owner: l.Owner, TopicID: l.SourceKey, OutputID: l.ToFormat}
		if !l.Enabled {
			f.Status = "paused"
		}
		f.DestName = ip
		if s.peers != nil && s.peers.Enabled() {
			if p := s.peers.ByIP(ctx, ip); p != nil {
				f.DestName = p.Name
			}
		}
		o := feedOut{Feed: f, TopicLabel: l.SourceLabel, OutputLabel: l.OutputLabel, ToFormat: l.ToFormat,
			DataClass: l.DataClass, Kind: "declared", Delivery: ds.View, ControlledBy: l.ControlledBy, Manifest: ds.Source,
			Sources: l.Sources}
		if o.OutputLabel == "" {
			o.OutputLabel = l.ToFormat
		}
		if o.DataClass == "" {
			o.DataClass = "real"
		}
		o.Integrated, o.Flowing = linkSignals(f, ds.View, false, l.Direction == "in", now)
		if !l.Enabled {
			o.Integrated = no("Switched off in its declaration.")
		}
		out = append(out, o)
	}
	return out
}
