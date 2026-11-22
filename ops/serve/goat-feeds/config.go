// SPDX-License-Identifier: Apache-2.0

package main

import (
	"encoding/json"
	"fmt"
	"net/netip"
	"os"
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
}

func (t *Topic) Output(id string) *Output {
	for i := range t.Outputs {
		if t.Outputs[i].ID == id {
			return &t.Outputs[i]
		}
	}
	return nil
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

	allowed netip.Prefix
}

func (c *Config) Topic(id string) *Topic {
	for i := range c.Catalog {
		if c.Catalog[i].ID == id {
			return &c.Catalog[i]
		}
	}
	return nil
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
	return c, nil
}

func LoadConfig(path string) (*Config, error) {
	b, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	return ParseConfig(b)
}
