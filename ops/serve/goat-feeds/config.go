// SPDX-License-Identifier: Apache-2.0

package main

import (
	"encoding/json"
	"fmt"
	"net/netip"
	"os"
	"sync"
)

// Output is one output form a topic offers: a goat-xf conversion and its framing.
type Output struct {
	ID       string            `json:"id"`
	Label    string            `json:"label"`
	ToFormat string            `json:"to_format"`
	Frame    string            `json:"frame,omitempty"`
	Set      map[string]string `json:"set,omitempty"`
}

// Topic is one catalog entry: what may be requested at this site.
type Topic struct {
	ID          string   `json:"id"`
	Label       string   `json:"label"`
	Key         string   `json:"key"`
	Kind        string   `json:"kind"`       // raw | processed
	DataClass   string   `json:"data_class"` // real | synthetic
	FromFormat  string   `json:"from_format"`
	InFrame     string   `json:"in_frame,omitempty"`
	Description string   `json:"description,omitempty"`
	Outputs     []Output `json:"outputs"`
	// Where the topic's data comes from (facets, flow view): site and system as named in
	// the estate.
	Sources []SourceRef `json:"sources,omitempty"`
	// A received feed's real-or-simulated declaration (sim_status.go), so whoever requests
	// it out sees it.
	SimStatus *SimStatus `json:"sim_status,omitempty"`
}

func (t *Topic) Output(id string) *Output {
	for i := range t.Outputs {
		if t.Outputs[i].ID == id {
			return &t.Outputs[i]
		}
	}
	return nil
}

// IntakeFormat is a standard a sending system may use: how goat-xf reads it.
type IntakeFormat struct {
	ID         string            `json:"id"`
	Label      string            `json:"label"`
	FromFormat string            `json:"from_format"`
	InFrame    string            `json:"in_frame,omitempty"`
	Set        map[string]string `json:"set,omitempty"`
	Note       string            `json:"note,omitempty"`
	// ReceiveTime names the model key given the time a message arrived, for a standard
	// that carries no absolute time of its own (MAVLink: boot-relative; CAT062: time of
	// day only). Empty: the message's own time is used.
	ReceiveTime string `json:"receive_time,omitempty"`
	// ToFormat overrides the site's canonical form for this standard, for example
	// goat-processors-report for a standard whose sender tags friend or hostile (CoT,
	// SAPIENT): that form carries the CoT type, the system track form does not.
	ToFormat string `json:"to_format,omitempty"`
}

// PublishAs is the form a feed of this standard is published in.
func (in *Intake) PublishAs(ft *IntakeFormat) string {
	if ft != nil && ft.ToFormat != "" {
		return ft.ToFormat
	}
	return in.ToFormat
}

// Intake is the receive side: systems send to a port on this core node, and each received
// feed is published on the fabric in one canonical form and offered in the catalog.
type Intake struct {
	ListenIP  string         `json:"listen_ip"`
	Ports     []int          `json:"ports"`
	KeyPrefix string         `json:"key_prefix"`
	ToFormat  string         `json:"to_format"`
	OutFrame  string         `json:"out_frame"`
	Formats   []IntakeFormat `json:"formats"`
	Outputs   []Output       `json:"outputs"`
}

func (in *Intake) Enabled() bool { return in != nil && in.ListenIP != "" && len(in.Ports) > 0 }

func (in *Intake) Format(id string) *IntakeFormat {
	for i := range in.Formats {
		if in.Formats[i].ID == id {
			return &in.Formats[i]
		}
	}
	return nil
}

func (in *Intake) HasPort(p int) bool {
	for _, q := range in.Ports {
		if q == p {
			return true
		}
	}
	return false
}

type PipeConfig struct {
	Binary      string `json:"binary"`
	Packs       string `json:"packs"`
	ZenohConfig string `json:"zenoh_config"`
}

type PeersConfig struct {
	API       string `json:"api"`
	TokenFile string `json:"token_file"`
}

// Config is the site's configuration, one JSON file rendered by the goat-feeds role.
type Config struct {
	Site                 string      `json:"site"`
	Goatnet              string      `json:"goatnet"`
	Listen               string      `json:"listen"`
	DBDSNFile            string      `json:"db_dsn_file"`
	PlatformGroups       []string    `json:"platform_groups"`
	TacticalGroups       []string    `json:"tactical_groups"`
	SelfServiceSynthetic bool        `json:"self_service_synthetic"`
	PortMin              int         `json:"port_min"`
	PortMax              int         `json:"port_max"`
	DefaultDays          int         `json:"default_days"`
	MaxDays              int         `json:"max_days"`
	AllowedCIDR          string      `json:"allowed_cidr"`
	Pipe                 PipeConfig  `json:"pipe"`
	Peers                PeersConfig `json:"peers"`
	Catalog              []Topic     `json:"catalog"`
	Intake               *Intake     `json:"intake"`
	// Declared links (ADR 1260): manifests written by the roles that start links.
	DeclaredDir   string        `json:"declared_dir"`
	DeclaredProbe DeclaredProbe `json:"declared_probe"`
	// Facets (fabric-feeds.md §11): the goatnet's estate file, receiving systems it does
	// not list, and the goatnet's mesh group prefix (stripped from role names).
	EstateFile  string       `json:"estate_file"`
	Systems     []SystemDecl `json:"systems"`
	GroupPrefix string       `json:"group_prefix"`
	// Tidy up (tidy.go): the functions offered in the role picker besides those already
	// in use, and the mesh groups whose peers the estate must list (empty: any peer a
	// role group places).
	Roles              []string `json:"roles"`
	TacticalPeerGroups []string `json:"tactical_peer_groups"`

	allowed netip.Prefix

	pubMu     sync.RWMutex
	published []Topic // received feeds now on the fabric, offered like catalog topics
}

func (c *Config) Topic(id string) *Topic {
	for i := range c.Catalog {
		if c.Catalog[i].ID == id {
			return &c.Catalog[i]
		}
	}
	c.pubMu.RLock()
	defer c.pubMu.RUnlock()
	for i := range c.published {
		if c.published[i].ID == id {
			t := c.published[i]
			return &t
		}
	}
	return nil
}

// AllTopics is the catalog plus the received feeds currently published.
func (c *Config) AllTopics() []Topic {
	c.pubMu.RLock()
	defer c.pubMu.RUnlock()
	return append(append([]Topic{}, c.Catalog...), c.published...)
}

// SetPublished replaces the published topics. A topic stays offered while its received feed
// is live (on or paused), so a pause does not break the feeds that read it.
func (c *Config) SetPublished(ts []Topic) {
	c.pubMu.Lock()
	defer c.pubMu.Unlock()
	c.published = ts
}

func ParseConfig(b []byte) (*Config, error) {
	c := &Config{Listen: "127.0.0.1:8098", PortMin: 1024, PortMax: 65535, DefaultDays: 30, MaxDays: 180,
		AllowedCIDR: "100.64.0.0/10", SelfServiceSynthetic: true, PlatformGroups: []string{"PLATFORM-ADMIN"},
		Pipe: PipeConfig{Binary: "/opt/goat-xf/current/goat-xf-pipe-zenoh", Packs: "/opt/goat-xf/current/packs",
			ZenohConfig: "/etc/goat-xf/zenoh-client.json5"}}
	if err := json.Unmarshal(b, c); err != nil {
		return nil, fmt.Errorf("config: %w", err)
	}
	if c.Site == "" {
		return nil, fmt.Errorf("config: site is required")
	}
	if c.Goatnet == "" {
		c.Goatnet = c.Site
	}
	if !(0 < c.PortMin && c.PortMin <= c.PortMax && c.PortMax <= 65535) {
		return nil, fmt.Errorf("config: port_min/port_max out of range")
	}
	if !(0 < c.DefaultDays && c.DefaultDays <= c.MaxDays) {
		return nil, fmt.Errorf("config: default_days must be between 1 and max_days")
	}
	p, err := netip.ParsePrefix(c.AllowedCIDR)
	if err != nil {
		return nil, fmt.Errorf("config: allowed_cidr: %w", err)
	}
	c.allowed = p
	seen := map[string]bool{}
	for i, t := range c.Catalog {
		w := fmt.Sprintf("catalog[%d]", i)
		if t.ID == "" || t.Key == "" || t.FromFormat == "" {
			return nil, fmt.Errorf("%s: id, key and from_format are required", w)
		}
		if seen[t.ID] {
			return nil, fmt.Errorf("%s: duplicate topic id %q", w, t.ID)
		}
		seen[t.ID] = true
		if t.Kind != "raw" && t.Kind != "processed" {
			return nil, fmt.Errorf("%s: kind must be raw or processed", w)
		}
		if t.DataClass != "real" && t.DataClass != "synthetic" {
			return nil, fmt.Errorf("%s: data_class must be real or synthetic", w)
		}
		if len(t.Outputs) == 0 {
			return nil, fmt.Errorf("%s: at least one output is required", w)
		}
		for j, o := range t.Outputs {
			if o.ID == "" || o.ToFormat == "" {
				return nil, fmt.Errorf("%s.outputs[%d]: id and to_format are required", w, j)
			}
			if o.Label == "" {
				c.Catalog[i].Outputs[j].Label = o.ID
			}
		}
		if t.Label == "" {
			c.Catalog[i].Label = t.ID
		}
	}
	if c.Intake != nil && (c.Intake.ListenIP != "" || len(c.Intake.Ports) > 0) {
		in := c.Intake
		if _, err := netip.ParseAddr(in.ListenIP); err != nil {
			return nil, fmt.Errorf("config: intake.listen_ip: %w", err)
		}
		if len(in.Ports) == 0 || len(in.Formats) == 0 || len(in.Outputs) == 0 {
			return nil, fmt.Errorf("config: intake needs ports, formats and outputs")
		}
		if in.KeyPrefix == "" {
			return nil, fmt.Errorf("config: intake.key_prefix is required")
		}
		if in.ToFormat == "" {
			in.ToFormat = "goat-processors-system-track"
		}
		if in.OutFrame == "" {
			in.OutFrame = "newline"
		}
		for i, f := range in.Formats {
			if f.ID == "" || f.FromFormat == "" {
				return nil, fmt.Errorf("config: intake.formats[%d]: id and from_format are required", i)
			}
			if f.Label == "" {
				in.Formats[i].Label = f.ID
			}
		}
		for j, o := range in.Outputs {
			if o.ID == "" || o.ToFormat == "" {
				return nil, fmt.Errorf("config: intake.outputs[%d]: id and to_format are required", j)
			}
			if o.Label == "" {
				in.Outputs[j].Label = o.ID
			}
		}
	}
	return c, nil
}

func LoadConfig(path string) (*Config, error) {
	b, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	return ParseConfig(b)
}

// DeclaredProbe names the commands that read a declared link's unit; empty means the
// system's systemctl and journalctl.
type DeclaredProbe struct {
	Systemctl  string `json:"systemctl"`
	Journalctl string `json:"journalctl"`
}

func (p DeclaredProbe) probe() ExecProbe {
	e := ExecProbe{Systemctl: p.Systemctl, Journalctl: p.Journalctl}
	if e.Systemctl == "" {
		e.Systemctl = "systemctl"
	}
	if e.Journalctl == "" {
		e.Journalctl = "journalctl"
	}
	return e
}
