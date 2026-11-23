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
	Peer       string `json:"peer,omitempty"`
	DutyCycled bool   `json:"duty_cycled,omitempty"`
}

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
	systems     map[string]SystemDecl // "<site>/<name>"
	byPeer      map[string]SystemDecl // first DNS label of the mesh peer, lower case
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

// LoadEstate reads the estate file (may be empty) and adds the extra systems.
func LoadEstate(path string, extra []SystemDecl) (*Estate, error) {
	e := &Estate{regionLabel: map[string]string{}, siteLabel: map[string]string{}, siteRegion: map[string]string{},
		systems: map[string]SystemDecl{}, byPeer: map[string]SystemDecl{}}
	if path != "" {
		b, err := os.ReadFile(path)
		if err != nil {
			return nil, err
		}
		var f estateFile
		if err := json.Unmarshal(b, &f); err != nil {
			return nil, err
		}
		for _, r := range f.Regions {
			e.regionLabel[r.ID] = r.Label
		}
		for _, s := range f.Sites {
			e.siteLabel[s.ID], e.siteRegion[s.ID] = s.Label, s.Region
			for _, sys := range s.Systems {
				sys.Site = s.ID
				e.add(sys)
			}
		}
	}
	for _, sys := range extra {
		e.add(sys)
	}
	e.empty = len(e.siteLabel) == 0 && len(e.systems) == 0
	return e, nil
}

func (e *Estate) add(s SystemDecl) {
	if s.Label == "" {
		s.Label = s.Name
	}
	e.systems[s.Site+"/"+s.Name] = s
	if s.Peer != "" {
		e.byPeer[peerLabel(s.Peer)] = s
	}
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
	Tier          string   `json:"tier"`
	Format        string   `json:"format"`
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
	for _, s := range sources {
		if s.Site == "" {
			known, duty = false, false
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
		} else {
			duty = false
			if s.System != "" {
				known = false
			}
		}
		f.SourceSystems = append(f.SourceSystems, name)
	}
	f.SourceSites, f.SourceSystems, f.SourceRegions = uniq(f.SourceSites), uniq(f.SourceSystems), uniq(f.SourceRegions)
	f.SourcesKnown, f.DutyCycled = known, duty
	// Destination (for an IN feed: the sending system).
	peerName := o.DestName
	if destPeer != nil {
		f.DestRole = roleFromGroups(destPeer.Groups, goatnetPrefix)
		f.DestSystem, peerName = destPeer.Name, destPeer.Name
	}
	for _, cand := range []string{peerName, o.DestName, o.DestIP} {
		if sys, ok := e.ByPeer(cand); ok {
			f.DestKnown, f.DestSystem, f.DestClass = true, sys.Label, sys.Class
			f.DestSite = e.SiteLabel(sys.Site)
			if f.DestSite == "" {
				f.DestSite = sys.Site
			}
			_, f.DestRegion = e.RegionOf(sys.Site)
			break
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

// stateWord is the row's state for filters and counts: attention, sending, at-rest,
// idle (integrated, nothing sent), off (not integrated, by choice).
func stateWord(o feedOut) string {
	switch {
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
