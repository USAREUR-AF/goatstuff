// SPDX-License-Identifier: Apache-2.0
package main

// Facets (ADR 1260, fabric-feeds.md §11): every link carries the same small set of
// facets so many sites and systems can be grouped, collapsed and filtered. Region, site
// and system come from the goatnet's estate declaration (goat-map's estate file: regions,
// sites, systems with their mesh peer, class and duty cycle). Receiving systems the
// estate does not list (operator consoles) are declared in this service's settings
// (`systems`), each placed in an estate site. A source or destination found in neither
// is shown as undeclared; the view never guesses from a label.

import (
	"encoding/json"
	"net"
	"os"
	"sort"
	"strings"
)

// SourceRef names where a link's data comes from.
type SourceRef struct {
	Label  string `json:"label"`
	Site   string `json:"site,omitempty"`
	System string `json:"system,omitempty"`
	// SiteLabel is filled in from the estate when the row is served.
	SiteLabel string `json:"site_label,omitempty"`
}

// UnmarshalJSON accepts a bare label as well as an object.
func (s *SourceRef) UnmarshalJSON(b []byte) error {
	var label string
	if json.Unmarshal(b, &label) == nil {
		*s = SourceRef{Label: label}
		return nil
	}
	type plain SourceRef
	var p plain
	if err := json.Unmarshal(b, &p); err != nil {
		return err
	}
	*s = SourceRef(p)
	return nil
}

// SystemDecl is a system in the estate or in the settings' `systems` list.
type SystemDecl struct {
	Name       string `json:"name"`
	Label      string `json:"label,omitempty"`
	Site       string `json:"site,omitempty"`
	Class      string `json:"class"`
	Role       string `json:"role,omitempty"`
	Peer       string `json:"peer,omitempty"`
	DutyCycled bool   `json:"duty_cycled,omitempty"`
	// Lifecycle: "active" (the default when empty) or "archived". An archived system
	// never raises attention; Archive says how it is expected to come back.
	Status  string       `json:"status,omitempty"`
	Archive *ArchiveInfo `json:"archive,omitempty"`
	// FarSide says where a boundary node's data goes (class "boundary").
	FarSide *FarSide `json:"far_side,omitempty"`
	// PlacedBy is set when an admin's placement (the overlay) changed this system.
	PlacedBy string `json:"placed_by,omitempty"`
}

// Archived reports whether the system is archived.
func (s SystemDecl) Archived() bool { return s.Status == "archived" }

// IsBoundary reports whether the system is a boundary node.
func (s SystemDecl) IsBoundary() bool { return strings.EqualFold(s.Class, "boundary") }

// ArchiveInfo records why a system was archived and how it is expected to come back.
type ArchiveInfo struct {
	At        string `json:"at"`
	By        string `json:"by"`
	Operation string `json:"operation,omitempty"`
	// Returns: same-system-same-site | same-system-new-site | new-system | unknown.
	Returns string `json:"returns"`
	// Rejoin: peer-kept | rejoin-needed | unknown.
	Rejoin string `json:"rejoin"`
	Notes  string `json:"notes,omitempty"`
	// Restore lists the link ids (feed ids, or "declared:<id>") to switch back on.
	Restore []string `json:"restore,omitempty"`
}

// FarSide is where a boundary node sends data (or receives it from).
type FarSide struct {
	Label string `json:"label"`
	// Direction: out (data leaves through it), in (data arrives through it) or both.
	Direction string `json:"direction,omitempty"`
}

var (
	archiveReturns = []string{"same-system-same-site", "same-system-new-site", "new-system", "unknown"}
	archiveRejoin  = []string{"peer-kept", "rejoin-needed", "unknown"}
	farDirections  = []string{"", "out", "in", "both"}
)

// archiveNote is one sentence a person reads beside an archived system.
func archiveNote(s SystemDecl) string {
	a := s.Archive
	if a == nil {
		return "Archived."
	}
	ret := map[string]string{
		"same-system-same-site": "expected back as the same system at the same site",
		"same-system-new-site":  "expected back as the same system at a new site",
		"new-system":            "expected to be replaced by a new system",
	}[a.Returns]
	if ret == "" {
		ret = "not known how it comes back"
	}
	rej := map[string]string{
		"peer-kept":     "its mesh peer is kept",
		"rejoin-needed": "it must rejoin the mesh",
	}[a.Rejoin]
	if rej == "" {
		rej = "not known whether it must rejoin the mesh"
	}
	out := "Archived"
	if a.Operation != "" {
		out += " after " + a.Operation
	}
	if a.By != "" {
		out += " by " + a.By
	}
	if len(a.At) >= 10 {
		out += " on " + a.At[:10]
	}
	return out + ": " + ret + "; " + rej + "."
}

// boundaryPhrase names where data through a boundary node goes. The bare words
// "boundary node" never appear without the far side.
func boundaryPhrase(s SystemDecl, direction string) string {
	if s.FarSide == nil || strings.TrimSpace(s.FarSide.Label) == "" {
		return "Through boundary node " + s.Label + ": where the data goes is not declared"
	}
	if direction == "in" {
		return "From " + s.FarSide.Label + " through boundary node " + s.Label
	}
	return "To " + s.FarSide.Label + " through boundary node " + s.Label
}

const reasonBoundaryNoFarSide = "boundary node: where the data goes is not declared"

type estateFile struct {
	Regions []struct {
		ID    string `json:"region_id"`
		Label string `json:"label"`
	} `json:"regions"`
	Sites []struct {
		ID      string       `json:"site_id"`
		Label   string       `json:"label"`
		Region  string       `json:"region_id"`
		Systems []SystemDecl `json:"systems"`
	} `json:"sites"`
}

// Estate is the joined hierarchy: region -> site -> system.
type Estate struct {
	empty       bool
	regionLabel map[string]string
	siteLabel   map[string]string
	siteRegion  map[string]string
	siteOrder   []string
	decls       []SystemDecl          // in declaration order, file first then settings
	fromFile    map[string]bool       // system names that come from the estate file
	systems     map[string]SystemDecl // "<site>/<name>"
	byPeer      map[string]SystemDecl // first DNS label of the mesh peer, lower case
	byName      map[string]SystemDecl // system name, lower case
	raw         []byte                // the estate file as read, for the effective export
}

func peerLabel(p string) string {
	p = strings.ToLower(strings.TrimSpace(p))
	if net.ParseIP(p) != nil {
		return p
	}
	if i := strings.IndexByte(p, '.'); i >= 0 {
		p = p[:i]
	}
	return p
}

func newEstate() *Estate {
	return &Estate{regionLabel: map[string]string{}, siteLabel: map[string]string{}, siteRegion: map[string]string{},
		fromFile: map[string]bool{}, systems: map[string]SystemDecl{}, byPeer: map[string]SystemDecl{}, byName: map[string]SystemDecl{}}
}

// LoadEstate reads the estate file (may be empty) and adds the extra systems.
func LoadEstate(path string, extra []SystemDecl) (*Estate, error) {
	e := newEstate()
	if path != "" {
		b, err := os.ReadFile(path)
		if err != nil {
			return nil, err
		}
		var f estateFile
		if err := json.Unmarshal(b, &f); err != nil {
			return nil, err
		}
		e.raw = b
		for _, r := range f.Regions {
			e.regionLabel[r.ID] = r.Label
		}
		for _, s := range f.Sites {
			e.siteLabel[s.ID], e.siteRegion[s.ID] = s.Label, s.Region
			e.siteOrder = append(e.siteOrder, s.ID)
			for _, sys := range s.Systems {
				sys.Site = s.ID
				e.fromFile[sys.Name] = true
				e.decls = append(e.decls, sys)
			}
		}
	}
	e.decls = append(e.decls, extra...)
	e.index()
	return e, nil
}

// index rebuilds the lookup maps from decls.
func (e *Estate) index() {
	e.systems, e.byPeer, e.byName = map[string]SystemDecl{}, map[string]SystemDecl{}, map[string]SystemDecl{}
	for i := range e.decls {
		if e.decls[i].Label == "" {
			e.decls[i].Label = e.decls[i].Name
		}
		s := e.decls[i]
		e.systems[s.Site+"/"+s.Name] = s
		e.byName[strings.ToLower(s.Name)] = s
		if s.Peer != "" {
			e.byPeer[peerLabel(s.Peer)] = s
		}
	}
	e.empty = len(e.siteLabel) == 0 && len(e.systems) == 0
}

// Enforced: an estate is declared, so destinations must resolve to a declared system.
func (e *Estate) Enforced() bool { return e != nil && !e.empty && len(e.siteLabel) > 0 }

// SiteIDs are the declared site ids in file order.
func (e *Estate) SiteIDs() []string {
	if e == nil {
		return nil
	}
	return append([]string{}, e.siteOrder...)
}

// Systems are every declared system, in declaration order.
func (e *Estate) Systems() []SystemDecl {
	if e == nil {
		return nil
	}
	return append([]SystemDecl{}, e.decls...)
}

// ByName finds a system by its name.
func (e *Estate) ByName(name string) (SystemDecl, bool) {
	if e == nil || name == "" {
		return SystemDecl{}, false
	}
	s, ok := e.byName[strings.ToLower(strings.TrimSpace(name))]
	return s, ok
}

// Resolve finds the declared system for the first candidate that matches a mesh peer
// (name or address) or a system name.
func (e *Estate) Resolve(cands ...string) (SystemDecl, bool) {
	for _, c := range cands {
		if c == "" {
			continue
		}
		if s, ok := e.ByPeer(c); ok {
			return s, true
		}
		if s, ok := e.ByName(c); ok {
			return s, true
		}
	}
	return SystemDecl{}, false
}

// SiteLabel and RegionOf answer for a site id; unknown ids answer empty.
func (e *Estate) SiteLabel(id string) string {
	if e == nil {
		return ""
	}
	return e.siteLabel[id]
}
func (e *Estate) RegionOf(site string) (id, label string) {
	if e == nil {
		return "", ""
	}
	id = e.siteRegion[site]
	return id, e.regionLabel[id]
}
func (e *Estate) System(site, name string) (SystemDecl, bool) {
	if e == nil {
		return SystemDecl{}, false
	}
	s, ok := e.systems[site+"/"+name]
	return s, ok
}
func (e *Estate) ByPeer(name string) (SystemDecl, bool) {
	if e == nil || name == "" {
		return SystemDecl{}, false
	}
	s, ok := e.byPeer[peerLabel(name)]
	return s, ok
}

// Facets is what every link row carries for grouping and filtering.
type Facets struct {
	SourceSites   []string `json:"source_sites"`
	SourceSystems []string `json:"source_systems"`
	SourceRegions []string `json:"source_regions"`
	SourcesKnown  bool     `json:"sources_known"`
	DutyCycled    bool     `json:"duty_cycled"`
	DestSite      string   `json:"dest_site"`
	DestRegion    string   `json:"dest_region"`
	DestSystem    string   `json:"dest_system"`
	DestClass     string   `json:"dest_class"`
	DestRole      string   `json:"dest_role"`
	DestKnown     bool     `json:"dest_known"`
	// DestKey is the declared system's name (for tidy-up and archiving), DestPlacedBy
	// who placed it through the overlay, when an admin did.
	DestKey      string   `json:"dest_key,omitempty"`
	DestPlacedBy string   `json:"dest_placed_by,omitempty"`
	SourceKeys   []string `json:"source_keys,omitempty"`
	// Boundary: "To <far side> through boundary node <name>" when the destination is a
	// boundary node.
	Boundary string `json:"boundary,omitempty"`
	// Archived: the destination (or every source) is archived; the link never raises
	// attention and its state is "archived".
	Archived    bool   `json:"archived"`
	ArchiveNote string `json:"archive_note,omitempty"`
	Tier        string `json:"tier"`
	Format      string `json:"format"`
	// Attention: the link needs a person, with the reasons.
	Attention bool     `json:"attention"`
	Reasons   []string `json:"reasons"`
	AtRest    bool     `json:"at_rest"`
}

// roleFromGroups picks the receiving system's role from its mesh groups: the first group
// that is not the catch-all or a transport group, without the goatnet prefix.
func roleFromGroups(groups []string, prefix string) string {
	var roles []string
	for _, g := range groups {
		if g == "All" || strings.HasSuffix(g, "-P2P") || strings.HasSuffix(g, "-INFRA") {
			continue
		}
		r := g
		if prefix != "" {
			r = strings.TrimPrefix(r, prefix+"-")
		}
		roles = append(roles, r)
	}
	sort.Strings(roles)
	if len(roles) == 0 {
		return ""
	}
	return roles[0]
}

func uniq(xs []string) []string {
	seen := map[string]bool{}
	var out []string
	for _, x := range xs {
		if x != "" && !seen[x] {
			seen[x] = true
			out = append(out, x)
		}
	}
	return out
}

// facetsFor computes a row's facets. sources are the row's declared sources; destPeer is
// the receiving (OUT) or sending (IN) system's mesh peer, when known.
func facetsFor(e *Estate, o *feedOut, sources []SourceRef, destPeer *Peer, goatnetPrefix string) Facets {
	f := Facets{Tier: o.Kind, Format: o.OutputLabel}
	if o.Kind == "received" || o.Kind == "declared" {
		f.Tier = map[bool]string{true: "processed", false: "raw"}[len(sources) > 1]
	}
	// Sources.
	known, duty := len(sources) > 0, len(sources) > 0
	allArchived := len(sources) > 0
	var srcArchive SystemDecl
	for _, s := range sources {
		if s.Site == "" {
			known, duty, allArchived = false, false, false
			continue
		}
		label := e.SiteLabel(s.Site)
		if label == "" {
			known = false
			label = s.Site
		}
		f.SourceSites = append(f.SourceSites, label)
		if _, rl := e.RegionOf(s.Site); rl != "" {
			f.SourceRegions = append(f.SourceRegions, rl)
		}
		name := s.Label
		if sys, ok := e.System(s.Site, s.System); ok {
			name = sys.Label
			duty = duty && sys.DutyCycled
			f.SourceKeys = append(f.SourceKeys, sys.Name)
			if sys.Archived() {
				srcArchive = sys
			} else {
				allArchived = false
			}
		} else {
			duty, allArchived = false, false
			if s.System != "" {
				known = false
			}
		}
		f.SourceSystems = append(f.SourceSystems, name)
	}
	f.SourceSites, f.SourceSystems, f.SourceRegions = uniq(f.SourceSites), uniq(f.SourceSystems), uniq(f.SourceRegions)
	f.SourceKeys = uniq(f.SourceKeys)
	f.SourcesKnown, f.DutyCycled = known, duty
	// Destination (for an IN feed: the sending system).
	peerName := o.DestName
	if destPeer != nil {
		f.DestRole = roleFromGroups(destPeer.Groups, goatnetPrefix)
		f.DestSystem, peerName = destPeer.Name, destPeer.Name
	}
	var dest SystemDecl
	if sys, ok := e.Resolve(peerName, o.DestName, o.DestIP); ok {
		dest = sys
		f.DestKnown, f.DestSystem, f.DestClass, f.DestKey, f.DestPlacedBy = true, sys.Label, sys.Class, sys.Name, sys.PlacedBy
		if sys.Role != "" && f.DestRole == "" {
			f.DestRole = sys.Role
		}
		f.DestSite = e.SiteLabel(sys.Site)
		if f.DestSite == "" {
			f.DestSite = sys.Site
		}
		_, f.DestRegion = e.RegionOf(sys.Site)
		if sys.IsBoundary() {
			f.Boundary = boundaryPhrase(sys, o.Direction)
		}
	}
	if f.DestSystem == "" {
		f.DestSystem = o.DestName
	}
	// Attention.
	switch {
	case o.Status == "pending":
		f.Reasons = append(f.Reasons, "waiting for approval")
	case o.Status == "active" && !o.Integrated.On:
		f.Reasons = append(f.Reasons, "switched on but not integrated")
	case o.Integrated.On && !o.Flowing.On && f.DutyCycled:
		f.AtRest = true
	case o.Integrated.On && !o.Flowing.On:
		f.Reasons = append(f.Reasons, "integrated, nothing sent in the last minute")
	}
	estateOn := e != nil && !e.empty
	if estateOn && o.Status != "paused" && !f.SourcesKnown && o.Direction != "in" {
		f.Reasons = append(f.Reasons, "source not in the estate")
	}
	if estateOn && o.Status != "paused" && !f.DestKnown {
		if o.Direction == "in" {
			f.Reasons = append(f.Reasons, "sending system not in the estate")
		} else {
			f.Reasons = append(f.Reasons, "receiving system not in the estate")
		}
	}
	if f.DestKnown && dest.IsBoundary() && (dest.FarSide == nil || strings.TrimSpace(dest.FarSide.Label) == "") {
		f.Reasons = append(f.Reasons, reasonBoundaryNoFarSide)
	}
	// An archived system never raises attention: its links read "archived".
	switch {
	case f.DestKnown && dest.Archived():
		f.Archived, f.ArchiveNote = true, archiveNote(dest)
	case allArchived:
		f.Archived, f.ArchiveNote = true, archiveNote(srcArchive)
	}
	if f.Archived {
		f.Reasons, f.AtRest = nil, false
	}
	f.Attention = len(f.Reasons) > 0
	if f.Reasons == nil {
		f.Reasons = []string{}
	}
	return f
}

// labelSources returns a copy of sources with each site's estate label filled in.
func labelSources(e *Estate, sources []SourceRef) []SourceRef {
	if len(sources) == 0 {
		return nil
	}
	out := make([]SourceRef, len(sources))
	for i, s := range sources {
		s.SiteLabel = e.SiteLabel(s.Site)
		out[i] = s
	}
	return out
}

// facetValue returns the values a row has for one grouping or filter facet.
func facetValues(o feedOut, facet string) []string {
	f := o.Facets
	switch facet {
	case "site":
		return uniq(append(append([]string{}, f.SourceSites...), f.DestSite))
	case "source_site":
		return f.SourceSites
	case "region":
		return uniq(append(append([]string{}, f.SourceRegions...), f.DestRegion))
	case "system":
		return uniq(append(append([]string{}, f.SourceSystems...), f.DestSystem))
	case "source":
		return f.SourceSystems
	case "dest_site":
		return []string{f.DestSite}
	case "dest_system":
		return []string{f.DestSystem}
	case "dest_role":
		return []string{f.DestRole}
	case "dest_class":
		return []string{f.DestClass}
	case "format":
		return []string{f.Format}
	case "owner":
		return []string{o.Owner}
	case "kind":
		return []string{o.Kind}
	case "direction":
		return []string{o.Direction}
	case "state":
		return []string{stateWord(o)}
	}
	return nil
}

// stateWord is the row's state for filters and counts: archived, attention, sending,
// at-rest, idle (integrated, nothing sent), off (not integrated, by choice).
func stateWord(o feedOut) string {
	switch {
	case o.Facets.Archived:
		return "archived"
	case o.Facets.Attention:
		return "attention"
	case o.Flowing.On:
		return "sending"
	case o.Facets.AtRest:
		return "at-rest"
	case o.Integrated.On:
		return "idle"
	}
	return "off"
}
