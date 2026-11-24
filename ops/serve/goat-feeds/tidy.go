// SPDX-License-Identifier: Apache-2.0
package main

// Tidy up: the page and API that hand the clean-up of systems back to the tactical
// admin. It lists what has slipped out of the estate's organisation, each with its
// reason, and offers Place, Archive and Bring back. Placing writes the overlay
// (placements.go); every write is in the audit log.
//
// The declare rule: once an estate is declared, a new feed's destination (or sending
// system) must resolve to a declared system. A destination that does not is declared
// with the request (site, kind, function, label and a machine name that follows the
// naming rule, naming.go): an admin's declaration is placed at once; a member's lands in
// Tidy up as a proposal for an admin.

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"net/http"
	"net/netip"
	"sort"
	"strings"
	"time"
)

// estate is the effective estate: the estate file plus the overlay.
func (s *Server) estate() *Estate {
	s.ovMu.RLock()
	defer s.ovMu.RUnlock()
	if s.eff != nil {
		return s.eff
	}
	return s.est
}

func (s *Server) placements() []Placement {
	s.ovMu.RLock()
	defer s.ovMu.RUnlock()
	return append([]Placement{}, s.overlay...)
}

// reloadOverlay reads the overlay and rebuilds the effective estate.
func (s *Server) reloadOverlay(ctx context.Context) error {
	if s.pl == nil {
		return nil
	}
	ps, err := s.pl.ListPlacements(ctx)
	if err != nil {
		return err
	}
	eff := s.est.WithOverlay(ps)
	s.ovMu.Lock()
	s.overlay, s.eff = ps, eff
	s.ovMu.Unlock()
	return nil
}

// RunOverlay keeps the overlay fresh (another instance, or a database restore).
func (s *Server) RunOverlay(ctx context.Context, every time.Duration) {
	t := time.NewTicker(every)
	defer t.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-t.C:
			if err := s.reloadOverlay(ctx); err != nil {
				log.Printf("goat-feeds: overlay: %v", err)
			}
		}
	}
}

// knownRoles are the functions offered in the role picker: the settings' roles, the
// roles systems already have, and the roles mesh groups give peers.
func (s *Server) knownRoles(ctx context.Context) []string {
	var out []string
	out = append(out, s.cfg.Roles...)
	for _, d := range s.estate().Systems() {
		out = append(out, d.Role)
	}
	if s.peers != nil && s.peers.Enabled() {
		for _, p := range s.peers.All(ctx) {
			out = append(out, roleFromGroups(p.Groups, s.cfg.GroupPrefix))
		}
	}
	out = uniq(out)
	sort.Strings(out)
	return out
}

// knownClasses are the kinds offered in the kind picker.
func (s *Server) knownClasses() []string {
	out := []string{"console", "boundary", "radar", "node"}
	for _, d := range s.estate().Systems() {
		out = append(out, d.Class)
	}
	out = uniq(out)
	sort.Strings(out)
	return out
}

type siteRef struct {
	ID     string `json:"id"`
	Label  string `json:"label"`
	Region string `json:"region,omitempty"`
}

func (s *Server) siteRefs() []siteRef {
	e := s.estate()
	out := []siteRef{}
	for _, id := range e.SiteIDs() {
		_, rl := e.RegionOf(id)
		out = append(out, siteRef{ID: id, Label: e.SiteLabel(id), Region: rl})
	}
	return out
}

func (s *Server) nameTaken(name string) bool {
	if _, ok := s.estate().ByName(name); ok {
		return true
	}
	for _, p := range s.placements() {
		if p.Key == name || p.Name == name {
			return true
		}
	}
	return false
}

// naming publishes the naming rule; with ?name= (and ?site=) it checks a name, with
// ?kind=&site=&function= it suggests the first free one.
func (s *Server) naming(w http.ResponseWriter, r *http.Request, v *Viewer) {
	q := r.URL.Query()
	sites := s.estate().SiteIDs()
	body := map[string]any{"rule": NameRule, "pattern": NamePattern, "sites": s.siteRefs(),
		"roles": s.knownRoles(r.Context()), "classes": s.knownClasses(), "example": exampleName(sites),
		"enforced": s.estate().Enforced() && s.pl != nil,
		"returns":  archiveReturns, "rejoin": archiveRejoin}
	if q.Has("name") {
		body["check"] = CheckName(strings.TrimSpace(q.Get("name")), q.Get("site"), sites)
		body["taken"] = s.nameTaken(strings.TrimSpace(q.Get("name")))
	}
	if q.Has("kind") || q.Has("function") {
		body["suggestion"] = SuggestName(q.Get("kind"), q.Get("site"), q.Get("function"), s.nameTaken)
	}
	writeJSON(w, 200, body)
}

// systemView is a declared system as the page shows it.
type systemView struct {
	SystemDecl
	SiteLabel   string `json:"site_label"`
	Boundary    string `json:"boundary,omitempty"`
	ArchiveNote string `json:"archive_note,omitempty"`
}

func (s *Server) view(d SystemDecl, direction string) systemView {
	sv := systemView{SystemDecl: d, SiteLabel: s.estate().SiteLabel(d.Site)}
	if d.IsBoundary() {
		sv.Boundary = boundaryPhrase(d, direction)
	}
	if d.Archived() {
		sv.ArchiveNote = archiveNote(d)
	}
	return sv
}

// proposalFor is a member's pending declaration for a mesh peer or address.
func (s *Server) proposalFor(cands ...string) *Placement {
	for _, p := range s.placements() {
		if p.State != "proposed" || p.Peer == "" {
			continue
		}
		for _, c := range cands {
			if c != "" && peerLabel(c) == peerLabel(p.Peer) {
				pp := p
				return &pp
			}
		}
	}
	return nil
}

// resolveSystem answers whether a destination is a declared system.
func (s *Server) resolveSystem(w http.ResponseWriter, r *http.Request, v *Viewer) {
	q := r.URL.Query()
	name, ip := strings.TrimSpace(q.Get("name")), strings.TrimSpace(q.Get("ip"))
	peer := ""
	if p := s.peerByIP(ip); p != nil {
		peer = p.Name
	}
	e := s.estate()
	body := map[string]any{"enforced": e.Enforced() && s.pl != nil, "found": false, "peer": peer}
	// An address off the mesh is refused when the request is sent; there is nothing to declare.
	if a, err := netip.ParseAddr(ip); err == nil && !s.cfg.allowed.Contains(a) {
		body["enforced"], body["off_mesh"] = false, true
	}
	if d, ok := e.Resolve(peer, name, ip); ok {
		body["found"], body["system"] = true, s.view(d, q.Get("direction"))
	} else if p := s.proposalFor(peer, ip); p != nil {
		body["proposal"] = p
	}
	writeJSON(w, 200, body)
}

// DeclareReq declares a system with a feed request or a Place action.
type DeclareReq struct {
	Name    string   `json:"name"`
	Site    string   `json:"site"`
	Class   string   `json:"class"`
	Role    string   `json:"role"`
	Label   string   `json:"label"`
	Peer    string   `json:"peer,omitempty"`
	FarSide *FarSide `json:"far_side,omitempty"`
	Reason  string   `json:"reason,omitempty"`
}

// checkDeclare validates a new system's declaration and returns its placement.
func (s *Server) checkDeclare(d DeclareReq) (Placement, error) {
	e := s.estate()
	sites := e.SiteIDs()
	d.Name, d.Site, d.Class, d.Role, d.Label = strings.TrimSpace(d.Name), strings.TrimSpace(d.Site), Token(d.Class), strings.TrimSpace(d.Role), strings.TrimSpace(d.Label)
	if e.SiteLabel(d.Site) == "" {
		return Placement{}, refuse(400, "Choose the site the system is at (one of: %s).", strings.Join(sites, ", "))
	}
	if d.Class == "" {
		return Placement{}, refuse(400, "Choose what kind of system it is (for example %s).", strings.Join(s.knownClasses(), ", "))
	}
	if Token(d.Role) == "" {
		return Placement{}, refuse(400, "Choose the system's function (for example ops, cue or relay), so it is grouped with systems that do the same job.")
	}
	if c := CheckName(d.Name, d.Site, sites); !c.OK {
		return Placement{}, refuse(400, "The machine name does not follow the naming rule %s %s Suggested: %s.",
			"<kind>-<site>-<function>-<nn>.", strings.Join(c.Problems, " "), SuggestName(d.Class, d.Site, d.Role, s.nameTaken))
	} else if c.Kind != d.Class {
		return Placement{}, refuse(400, "The machine name starts with %q but the system is a %s; start it with %s-.", c.Kind, d.Class, d.Class)
	}
	if d.Label == "" {
		d.Label = d.Name
	}
	if !nameRE.MatchString(d.Label) {
		return Placement{}, refuse(400, "Give the system a display label people know it by (letters, digits, spaces, . _ - / ( )).")
	}
	if d.Class == "boundary" {
		if d.FarSide == nil || strings.TrimSpace(d.FarSide.Label) == "" {
			return Placement{}, refuse(400, "Say where data through this boundary node goes (the far side), for example \"Partner network\". The words \"boundary node\" alone do not tell people where their data goes.")
		}
	}
	if d.FarSide != nil {
		d.FarSide.Label = strings.TrimSpace(d.FarSide.Label)
		if !inList(farDirections, d.FarSide.Direction) {
			return Placement{}, refuse(400, "The far side's direction is out, in or both.")
		}
		if d.FarSide.Label == "" {
			d.FarSide = nil
		}
	}
	return Placement{Key: d.Name, Site: d.Site, Class: d.Class, Role: d.Role, Label: d.Label, Peer: strings.TrimSpace(d.Peer),
		FarSide: d.FarSide, Reason: strings.TrimSpace(d.Reason)}, nil
}

// declareForFeed applies the declare rule to a validated feed. It returns the placement
// to write once the feed exists (nil when the destination is declared already).
func (s *Server) declareForFeed(ctx context.Context, req FeedRequest, f Feed, v *Viewer) (*Placement, error) {
	e := s.estate()
	if s.pl == nil || !e.Enforced() {
		return nil, nil
	}
	peer := ""
	if p := s.peerByIP(f.DestIP); p != nil {
		peer = p.Name
	}
	if _, ok := e.Resolve(peer, f.DestName, f.DestIP); ok {
		return nil, nil
	}
	who := "receiving system"
	if f.Direction == "in" {
		who = "sending system"
	}
	prop := s.proposalFor(peer, f.DestIP)
	if prop != nil && !v.IsAdmin {
		return nil, nil // already proposed; it waits in Tidy up
	}
	if req.Declare == nil {
		return nil, refuse(400, "The %s %s (%s) is not a declared system. Declare it with the request: its site, kind, function and a machine name that follows %s (see GET /api/naming), or choose a declared system.",
			who, f.DestName, f.DestIP, "<kind>-<site>-<function>-<nn>")
	}
	d := *req.Declare
	if d.Peer == "" {
		d.Peer = peer
		if d.Peer == "" {
			d.Peer = f.DestIP
		}
	}
	p, err := s.checkDeclare(d)
	if err != nil {
		return nil, err
	}
	if s.nameTaken(p.Key) && (prop == nil || prop.Key != p.Key) {
		return nil, refuse(409, "%s is already the name of a declared system. Choose the next free number (suggested: %s).", p.Key,
			SuggestName(p.Class, p.Site, p.Role, s.nameTaken))
	}
	p.By, p.State = v.Email, "proposed"
	if v.IsAdmin {
		p.State = "placed"
	}
	return &p, nil
}

func (s *Server) writePlacement(ctx context.Context, p Placement, action, detail string) error {
	if err := s.pl.PutPlacement(ctx, p, action, detail); err != nil {
		return err
	}
	return s.reloadOverlay(ctx)
}

func placementDetail(p Placement) string {
	parts := []string{}
	add := func(k, v string) {
		if v != "" {
			parts = append(parts, k+"="+v)
		}
	}
	add("name", p.Name)
	add("site", p.Site)
	add("kind", p.Class)
	add("function", p.Role)
	add("label", p.Label)
	add("peer", p.Peer)
	add("status", p.Status)
	if p.FarSide != nil {
		add("far side", p.FarSide.Label+" ("+p.FarSide.Direction+")")
	}
	if p.Archive != nil {
		add("returns", p.Archive.Returns)
		add("rejoin", p.Archive.Rejoin)
		add("operation", p.Archive.Operation)
		if len(p.Archive.Restore) > 0 {
			add("restore", strings.Join(p.Archive.Restore, ","))
		}
	}
	add("reason", p.Reason)
	return strings.Join(parts, "; ")
}

// ---------- tidy list ----------

// TidyLink is one link a tidy item concerns.
type TidyLink struct {
	ID           string `json:"id"`
	Title        string `json:"title"`
	Kind         string `json:"kind"`
	Status       string `json:"status"`
	On           bool   `json:"on"`
	ControlledBy string `json:"controlled_by,omitempty"`
	Manifest     string `json:"manifest,omitempty"`
}

// TidyItem is one thing that needs organising, with its reason and the actions offered.
type TidyItem struct {
	Kind      string      `json:"kind"`
	Key       string      `json:"key,omitempty"`  // the declared system's name
	Peer      string      `json:"peer,omitempty"` // a mesh peer or address not yet declared
	Title     string      `json:"title"`
	Reason    string      `json:"reason"`
	Suggested string      `json:"suggested_name,omitempty"`
	System    *systemView `json:"system,omitempty"`
	Proposal  *Placement  `json:"proposal,omitempty"`
	Links     []TidyLink  `json:"links"`
	Actions   []string    `json:"actions"`
}

func linkTitle(o feedOut) string {
	if o.Direction == "in" {
		return o.TopicLabel + " ← " + o.DestName
	}
	t := o.TopicLabel
	if t == "" {
		t = o.TopicID
	}
	return t + " → " + o.DestName
}

func linkOn(o feedOut) bool {
	return o.Status == "active" || o.Status == "pending"
}

func tidyLink(o feedOut) TidyLink {
	return TidyLink{ID: o.ID, Title: linkTitle(o), Kind: o.Kind, Status: o.Status, On: linkOn(o), ControlledBy: o.ControlledBy, Manifest: o.Manifest}
}

// liveRows are every requested feed that still counts, and every declared link.
func (s *Server) liveRows(ctx context.Context, v *Viewer) ([]feedOut, error) {
	feeds, err := s.reg.List(ctx, liveStates...)
	if err != nil {
		return nil, err
	}
	rows := make([]feedOut, 0, len(feeds))
	for _, f := range feeds {
		rows = append(rows, s.decorate(f, v, false))
	}
	return append(rows, s.declaredRows(ctx, time.Now())...), nil
}

// linksOf are the rows that send to, receive from, or carry data of a system.
func linksOf(rows []feedOut, name string) []feedOut {
	var out []feedOut
	for _, o := range rows {
		if o.Facets.DestKey == name || inList(o.Facets.SourceKeys, name) {
			out = append(out, o)
		}
	}
	return out
}

// sysLinks is a declared system with its links (Tidy up's archive picker and archived list).
type sysLinks struct {
	systemView
	Links []TidyLink `json:"links"`
}

func (s *Server) tidyItems(ctx context.Context, v *Viewer) ([]TidyItem, []sysLinks, []sysLinks, error) {
	rows, err := s.liveRows(ctx, v)
	if err != nil {
		return nil, nil, nil, err
	}
	e := s.estate()
	sites := e.SiteIDs()
	items := []TidyItem{}
	undeclared := map[string]*TidyItem{} // by peer label
	order := []string{}
	addUndeclared := func(peer, title, reason string) *TidyItem {
		k := peerLabel(peer)
		if it, ok := undeclared[k]; ok {
			return it
		}
		it := &TidyItem{Kind: "undeclared", Peer: peer, Title: title, Reason: reason, Links: []TidyLink{}, Actions: []string{"place"}}
		if p := s.proposalFor(peer); p != nil {
			it.Proposal = p
		}
		undeclared[k] = it
		order = append(order, k)
		return it
	}
	if e.Enforced() {
		for _, o := range rows {
			if o.Facets.DestKnown || o.Status == "paused" && o.Kind != "declared" {
				continue
			}
			peer := o.DestIP
			if p := s.peerByIP(o.DestIP); p != nil {
				peer = p.Name
			}
			who := "receives"
			if o.Direction == "in" {
				who = "sends"
			}
			it := addUndeclared(peer, fmt.Sprintf("%s (%s) is not in the estate", o.DestName, o.DestIP),
				fmt.Sprintf("It %s on links, but no site or function is declared for it, so its links cannot be grouped or checked. Place it in its site and function.", who))
			it.Links = append(it.Links, tidyLink(o))
		}
		// Mesh peers in tactical groups that the estate does not list.
		if s.peers != nil && s.peers.Enabled() {
			for _, p := range s.peers.All(ctx) {
				if !s.tacticalPeer(p) {
					continue
				}
				if _, ok := e.Resolve(p.Name, p.IP); ok {
					continue
				}
				addUndeclared(p.Name, fmt.Sprintf("%s (%s) is on the mesh but not in the estate", p.Name, p.IP),
					fmt.Sprintf("It is in the mesh groups %s, which are tactical, but no site or function is declared for it. Place it in its site and function; if it is gone, place it and then archive it.", strings.Join(p.Groups, ", ")))
			}
		}
	}
	for _, k := range order {
		it := undeclared[k]
		if it.Proposal != nil {
			it.Kind = "proposed"
			it.Title = fmt.Sprintf("%s asked to declare %s at %s", it.Proposal.By, it.Proposal.Key, e.SiteLabel(it.Proposal.Site))
			it.Reason = "A member declared this system with a feed request. Check its site, function and name, then place it, or dismiss the proposal."
			it.Actions = []string{"place", "dismiss"}
		}
		items = append(items, *it)
	}
	// Proposals whose peer appears on no link and in no tactical group.
	for _, p := range s.placements() {
		if p.State != "proposed" {
			continue
		}
		if _, ok := undeclared[peerLabel(p.Peer)]; ok {
			continue
		}
		pp := p
		items = append(items, TidyItem{Kind: "proposed", Peer: p.Peer, Proposal: &pp, Links: []TidyLink{}, Actions: []string{"place", "dismiss"},
			Title:  fmt.Sprintf("%s asked to declare %s at %s", p.By, p.Key, e.SiteLabel(p.Site)),
			Reason: "A member declared this system with a feed request. Check its site, function and name, then place it, or dismiss the proposal."})
	}
	// Suggestions made in this listing count as taken, so two systems are never offered one name.
	offered := map[string]bool{}
	taken := func(n string) bool { return offered[n] || s.nameTaken(n) }
	archived, active := []sysLinks{}, []sysLinks{}
	for _, d := range e.Systems() {
		sv := s.view(d, "out")
		links := linksOf(rows, d.Name)
		all := []TidyLink{}
		for _, o := range links {
			all = append(all, tidyLink(o))
		}
		if d.Archived() {
			archived = append(archived, sysLinks{sv, all})
			var on []TidyLink
			for _, o := range links {
				if linkOn(o) {
					on = append(on, tidyLink(o))
				}
			}
			if len(on) > 0 {
				items = append(items, TidyItem{Kind: "archived-link-on", Key: d.Name, System: &sv, Links: on, Actions: []string{"restore"},
					Title:  fmt.Sprintf("%s is archived but %d of its links are still on", d.Label, len(on)),
					Reason: "An archived system's links should be off. Pause the requested feeds; switch declared links off in the file that controls them; or bring the system back."})
			}
			continue
		}
		active = append(active, sysLinks{sv, all})
		tl := all
		if d.IsBoundary() && (d.FarSide == nil || strings.TrimSpace(d.FarSide.Label) == "") {
			items = append(items, TidyItem{Kind: "boundary-far-side", Key: d.Name, System: &sv, Links: tl, Actions: []string{"place", "archive"},
				Title:  fmt.Sprintf("Where data through boundary node %s goes is not declared", d.Label),
				Reason: "People sending to it cannot tell where their data ends up. Declare its far side, for example \"Partner network\"."})
		}
		if c := CheckName(d.Name, d.Site, sites); !c.OK {
			items = append(items, TidyItem{Kind: "name-rule", Key: d.Name, System: &sv, Links: tl, Actions: []string{"place", "archive"},
				Suggested: suggestOnce(d, taken, offered),
				Title:     fmt.Sprintf("%s does not follow the naming rule", d.Name),
				Reason:    strings.Join(c.Problems, " ") + " Rename it here, and rename the machine at the mesh (NetBird) stage to match."})
		}
	}
	return items, active, archived, nil
}

func firstNonEmpty(xs ...string) string {
	for _, x := range xs {
		if x != "" {
			return x
		}
	}
	return ""
}

// tacticalPeer: a mesh peer the estate should list. With tactical_peer_groups set, a
// peer in one of them; otherwise any peer a role group places.
func (s *Server) tacticalPeer(p Peer) bool {
	if len(s.cfg.TacticalPeerGroups) > 0 {
		return anyIn(p.Groups, s.cfg.TacticalPeerGroups)
	}
	return roleFromGroups(p.Groups, s.cfg.GroupPrefix) != ""
}

func (s *Server) tidy(w http.ResponseWriter, r *http.Request, v *Viewer) {
	if !v.IsAdmin {
		writeErr(w, 403, "Tidy up is for tactical and platform admins.")
		return
	}
	items, active, archived, err := s.tidyItems(r.Context(), v)
	if err != nil {
		s.fail(w, err)
		return
	}
	writeJSON(w, 200, map[string]any{"count": len(items), "items": items, "systems": active, "archived": archived,
		"overlay": s.placements(), "can_place": s.pl != nil, "rule": NameRule, "sites": s.siteRefs(),
		"roles": s.knownRoles(r.Context()), "classes": s.knownClasses(), "returns": archiveReturns, "rejoin": archiveRejoin})
}

func (s *Server) effectiveEstate(w http.ResponseWriter, r *http.Request, v *Viewer) {
	if !v.IsAdmin {
		writeErr(w, 403, "The effective estate is for tactical and platform admins.")
		return
	}
	b, err := s.est.EffectiveJSON(s.placements())
	if err != nil {
		s.fail(w, err)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	w.Header().Set("Content-Disposition", `inline; filename="estate.effective.json"`)
	_, _ = w.Write(b)
}

// ---------- actions on a system ----------

type systemBody struct {
	DeclareReq
	// Archive.
	Returns   string   `json:"returns"`
	Rejoin    string   `json:"rejoin"`
	Operation string   `json:"operation"`
	Notes     string   `json:"notes"`
	Restore   []string `json:"restore"`
	Pause     bool     `json:"pause"`
	// Bring back: resume the requested feeds in the restore list (default true).
	Resume *bool `json:"resume"`
}

func (s *Server) systemAction(w http.ResponseWriter, r *http.Request, v *Viewer) {
	if !v.IsAdmin {
		writeErr(w, 403, "Only a tactical or platform admin places, archives or brings back systems.")
		return
	}
	if s.pl == nil {
		writeErr(w, 503, "This site keeps no placements (no database store for them).")
		return
	}
	key := strings.TrimSpace(r.PathValue("key"))
	var body systemBody
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 64*1024)).Decode(&body); err != nil && err.Error() != "EOF" {
		writeErr(w, 400, "The request is not valid JSON.")
		return
	}
	var err error
	var out any
	switch r.PathValue("action") {
	case "place":
		out, err = s.place(r.Context(), key, body, v)
	case "archive":
		out, err = s.archive(r.Context(), key, body, v)
	case "restore":
		out, err = s.restore(r.Context(), key, body, v)
	case "dismiss":
		out, err = s.dismiss(r.Context(), key, body, v)
	default:
		writeErr(w, 404, "Unknown action: place, archive, restore or dismiss.")
		return
	}
	if err != nil {
		s.fail(w, err)
		return
	}
	writeJSON(w, 200, out)
}

// existing returns the overlay entry for key, or a fresh one.
func (s *Server) existing(ctx context.Context, key string) Placement {
	p, err := s.pl.GetPlacement(ctx, key)
	if err != nil {
		return Placement{Key: key}
	}
	return p
}

func (s *Server) place(ctx context.Context, key string, b systemBody, v *Viewer) (any, error) {
	e := s.estate()
	if key == "" {
		return nil, refuse(400, "Name the system to place.")
	}
	cur, declared := e.ByName(key)
	if !declared {
		// A new system: the key is its machine name; everything is required.
		d := b.DeclareReq
		d.Name = key
		p, err := s.checkDeclare(d)
		if err != nil {
			return nil, err
		}
		prev := s.existing(ctx, key)
		if prev.State == "" && s.nameTaken(key) {
			return nil, refuse(409, "%s is already the name of a declared system.", key)
		}
		p.By, p.State = v.Email, "placed"
		if err := s.writePlacement(ctx, p, "place", placementDetail(p)); err != nil {
			return nil, err
		}
		got, _ := s.estate().ByName(key)
		return map[string]any{"system": s.view(got, "out")}, nil
	}
	// A declared system: change only what is given.
	p := s.existing(ctx, key)
	sites := e.SiteIDs()
	if b.Site != "" {
		if e.SiteLabel(b.Site) == "" {
			return nil, refuse(400, "Choose the site the system is at (one of: %s).", strings.Join(sites, ", "))
		}
		p.Site = b.Site
	}
	if b.Class != "" {
		p.Class = Token(b.Class)
	}
	if b.Role != "" {
		p.Role = strings.TrimSpace(b.Role)
	}
	if b.Label != "" {
		if !nameRE.MatchString(strings.TrimSpace(b.Label)) {
			return nil, refuse(400, "Give the system a display label people know it by (letters, digits, spaces, . _ - / ( )).")
		}
		p.Label = strings.TrimSpace(b.Label)
	}
	if b.Peer != "" {
		p.Peer = strings.TrimSpace(b.Peer)
	}
	if n := strings.TrimSpace(b.Name); n != "" && n != key {
		site := firstNonEmpty(p.Site, cur.Site)
		if c := CheckName(n, site, sites); !c.OK {
			return nil, refuse(400, "The new machine name does not follow the naming rule: %s Suggested: %s.", strings.Join(c.Problems, " "),
				SuggestName(firstNonEmpty(p.Class, cur.Class), site, firstNonEmpty(p.Role, cur.Role, "ops"), s.nameTaken))
		}
		if s.nameTaken(n) {
			return nil, refuse(409, "%s is already the name of a declared system.", n)
		}
		p.Name = n
	}
	if b.FarSide != nil {
		fs := *b.FarSide
		fs.Label = strings.TrimSpace(fs.Label)
		if !inList(farDirections, fs.Direction) {
			return nil, refuse(400, "The far side's direction is out, in or both.")
		}
		p.FarSide = &fs
	}
	class := firstNonEmpty(p.Class, cur.Class)
	if strings.EqualFold(class, "boundary") {
		fs := p.FarSide
		if fs == nil {
			fs = cur.FarSide
		}
		if fs == nil || fs.Label == "" {
			return nil, refuse(400, "Say where data through this boundary node goes (the far side), for example \"Partner network\".")
		}
	}
	p.Key, p.By, p.State, p.Reason = key, v.Email, "placed", strings.TrimSpace(b.Reason)
	if err := s.writePlacement(ctx, p, "place", placementDetail(p)); err != nil {
		return nil, err
	}
	got, _ := s.estate().ByName(firstNonEmpty(p.Name, key))
	return map[string]any{"system": s.view(got, "out")}, nil
}

func (s *Server) archive(ctx context.Context, key string, b systemBody, v *Viewer) (any, error) {
	cur, ok := s.estate().ByName(key)
	if !ok {
		return nil, refuse(404, "No declared system is named %s. Place it first, or archive it in the estate file.", key)
	}
	if b.Returns == "" {
		b.Returns = "unknown"
	}
	if b.Rejoin == "" {
		b.Rejoin = "unknown"
	}
	if !inList(archiveReturns, b.Returns) {
		return nil, refuse(400, "Say how it comes back: %s.", strings.Join(archiveReturns, ", "))
	}
	if !inList(archiveRejoin, b.Rejoin) {
		return nil, refuse(400, "Say whether it must rejoin the mesh: %s.", strings.Join(archiveRejoin, ", "))
	}
	if len(b.Operation) > 120 || len(b.Notes) > 1000 {
		return nil, refuse(400, "Keep the operation under 120 characters and the notes under 1000.")
	}
	rows, err := s.liveRows(ctx, v)
	if err != nil {
		return nil, err
	}
	links := linksOf(rows, cur.Name)
	restore := b.Restore
	if restore == nil {
		for _, o := range links {
			if linkOn(o) {
				restore = append(restore, o.ID)
			}
		}
	}
	p := s.existing(ctx, key)
	p.Key, p.Status, p.By, p.State, p.Reason = key, "archived", v.Email, "placed", strings.TrimSpace(b.Notes)
	p.Archive = &ArchiveInfo{At: time.Now().UTC().Format(time.RFC3339), By: v.Email, Operation: strings.TrimSpace(b.Operation),
		Returns: b.Returns, Rejoin: b.Rejoin, Notes: strings.TrimSpace(b.Notes), Restore: restore}
	if err := s.writePlacement(ctx, p, "archive", placementDetail(p)); err != nil {
		return nil, err
	}
	paused, declared := []string{}, []TidyLink{}
	if b.Pause {
		for _, o := range links {
			if !inList(restore, o.ID) {
				continue
			}
			if o.Kind == "declared" {
				if o.Status == "active" {
					declared = append(declared, tidyLink(o))
				}
				continue
			}
			if o.Status == "active" && Can(v, "pause", o.Feed) {
				if _, err := s.reg.Transition(ctx, o.ID, "paused", v.Email, "pause", "system "+key+" archived", false, nil); err != nil {
					return nil, err
				}
				paused = append(paused, o.ID)
			}
		}
	}
	got, _ := s.estate().ByName(key)
	return map[string]any{"system": s.view(got, "out"), "paused": paused, "declared_still_on": declared}, nil
}

func (s *Server) restore(ctx context.Context, key string, b systemBody, v *Viewer) (any, error) {
	cur, ok := s.estate().ByName(key)
	if !ok {
		return nil, refuse(404, "No declared system is named %s.", key)
	}
	if !cur.Archived() {
		return nil, refuse(409, "%s is not archived.", key)
	}
	var restore []string
	if cur.Archive != nil {
		restore = cur.Archive.Restore
	}
	p := s.existing(ctx, key)
	p.Key, p.Status, p.Archive, p.By, p.State, p.Reason = key, "active", nil, v.Email, "placed", strings.TrimSpace(b.Reason)
	if err := s.writePlacement(ctx, p, "restore", placementDetail(p)+"; restore list "+strings.Join(restore, ",")); err != nil {
		return nil, err
	}
	resume := b.Resume == nil || *b.Resume
	resumed, declared := []string{}, []TidyLink{}
	type skip struct {
		ID  string `json:"id"`
		Why string `json:"why"`
	}
	skipped := []skip{}
	decl := map[string]feedOut{}
	for _, o := range s.declaredRows(ctx, time.Now()) {
		decl[o.ID] = o
	}
	for _, id := range restore {
		if strings.HasPrefix(id, "declared:") {
			if o, ok := decl[id]; ok {
				declared = append(declared, tidyLink(o))
			} else {
				skipped = append(skipped, skip{id, "no declared link has this id any more"})
			}
			continue
		}
		f, err := s.reg.Get(ctx, id)
		if errors.Is(err, ErrNotFound) {
			skipped = append(skipped, skip{id, "no such feed"})
			continue
		} else if err != nil {
			return nil, err
		}
		switch {
		case f.Status == "active" || f.Status == "pending":
			skipped = append(skipped, skip{id, "already " + f.Status})
		case f.Status != "paused":
			skipped = append(skipped, skip{id, "it is " + f.Status + "; request it again"})
		case !resume:
			skipped = append(skipped, skip{id, "left paused, as asked"})
		case !Can(v, "resume", f):
			skipped = append(skipped, skip{id, "you cannot resume it"})
		default:
			if _, err := s.reg.Transition(ctx, id, "active", v.Email, "resume", "system "+key+" brought back", false, nil); err != nil {
				return nil, err
			}
			resumed = append(resumed, id)
		}
	}
	got, _ := s.estate().ByName(key)
	return map[string]any{"system": s.view(got, "out"), "resumed": resumed, "declared": declared, "skipped": skipped}, nil
}

func (s *Server) dismiss(ctx context.Context, key string, b systemBody, v *Viewer) (any, error) {
	p, err := s.pl.GetPlacement(ctx, key)
	if err != nil {
		return nil, refuse(404, "No proposal or placement is named %s.", key)
	}
	note := strings.TrimSpace(b.Reason)
	if p.State == "placed" && note == "" {
		return nil, refuse(400, "Say why the placement is dismissed (for example: promoted into the estate file).")
	}
	if err := s.pl.DeletePlacement(ctx, key, v.Email, "dismiss", firstNonEmpty(note, "proposal by "+p.By+" dismissed")); err != nil {
		return nil, err
	}
	if err := s.reloadOverlay(ctx); err != nil {
		return nil, err
	}
	return map[string]any{"dismissed": key}, nil
}

func inList(xs []string, x string) bool {
	for _, y := range xs {
		if y == x {
			return true
		}
	}
	return false
}

func suggestOnce(d SystemDecl, taken func(string) bool, offered map[string]bool) string {
	n := SuggestName(d.Class, d.Site, firstNonEmpty(d.Role, "ops"), taken)
	offered[n] = true
	return n
}
