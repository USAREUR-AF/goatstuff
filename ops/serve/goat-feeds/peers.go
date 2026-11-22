// SPDX-License-Identifier: Apache-2.0

package main

import (
	"context"
	"encoding/json"
	"net/http"
	"os"
	"sort"
	"strings"
	"sync"
	"time"
)

// Peer is a mesh peer from the NetBird management API, for the destination picker.
type Peer struct {
	ID        string   `json:"id"`
	Name      string   `json:"name"`
	IP        string   `json:"ip"`
	Connected bool     `json:"connected"`
	Hostname  string   `json:"hostname"`
	OS        string   `json:"os"`
	Groups    []string `json:"groups"`
	LastSeen  string   `json:"last_seen"`
}

type Peers struct {
	api, tokenFile string
	ttl            time.Duration
	client         *http.Client

	mu      sync.Mutex
	cache   []Peer
	at      time.Time
	LastErr string
}

func NewPeers(api, tokenFile string) *Peers {
	return &Peers{api: strings.TrimRight(api, "/"), tokenFile: tokenFile, ttl: time.Minute,
		client: &http.Client{Timeout: 8 * time.Second}}
}

func (p *Peers) Enabled() bool { return p.api != "" && p.tokenFile != "" }

func (p *Peers) fetch(ctx context.Context) ([]Peer, error) {
	tok, err := os.ReadFile(p.tokenFile)
	if err != nil {
		return nil, err
	}
	req, _ := http.NewRequestWithContext(ctx, "GET", p.api+"/api/peers", nil)
	req.Header.Set("Authorization", "Token "+strings.TrimSpace(string(tok)))
	req.Header.Set("Accept", "application/json")
	resp, err := p.client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != 200 {
		return nil, &Refused{Msg: "NetBird API answered " + resp.Status, Status: 502}
	}
	var raw []struct {
		ID        string `json:"id"`
		Name      string `json:"name"`
		IP        string `json:"ip"`
		Connected bool   `json:"connected"`
		Hostname  string `json:"hostname"`
		OS        string `json:"os"`
		LastSeen  string `json:"last_seen"`
		Groups    []struct {
			Name string `json:"name"`
		} `json:"groups"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&raw); err != nil {
		return nil, err
	}
	out := make([]Peer, 0, len(raw))
	for _, r := range raw {
		pp := Peer{ID: r.ID, Name: r.Name, IP: r.IP, Connected: r.Connected, Hostname: r.Hostname, OS: r.OS, LastSeen: r.LastSeen}
		for _, g := range r.Groups {
			if g.Name != "All" {
				pp.Groups = append(pp.Groups, g.Name)
			}
		}
		out = append(out, pp)
	}
	sort.Slice(out, func(i, j int) bool { return strings.ToLower(out[i].Name) < strings.ToLower(out[j].Name) })
	return out, nil
}

func (p *Peers) All(ctx context.Context) []Peer {
	if !p.Enabled() {
		return nil
	}
	p.mu.Lock()
	defer p.mu.Unlock()
	if time.Since(p.at) > p.ttl {
		if peers, err := p.fetch(ctx); err != nil {
			p.LastErr = err.Error()
		} else {
			p.cache, p.at, p.LastErr = peers, time.Now(), ""
		}
	}
	return append([]Peer{}, p.cache...)
}

func (p *Peers) Search(ctx context.Context, q string, limit int) []Peer {
	q = strings.ToLower(strings.TrimSpace(q))
	out := []Peer{}
	for _, pp := range p.All(ctx) {
		if q == "" || strings.Contains(strings.ToLower(pp.Name), q) || strings.Contains(pp.IP, q) ||
			strings.Contains(strings.ToLower(pp.Hostname), q) {
			out = append(out, pp)
			if len(out) >= limit {
				break
			}
		}
	}
	return out
}

func (p *Peers) ByIP(ctx context.Context, ip string) *Peer {
	for _, pp := range p.All(ctx) {
		if pp.IP == ip {
			c := pp
			return &c
		}
	}
	return nil
}
