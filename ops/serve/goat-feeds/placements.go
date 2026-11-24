// SPDX-License-Identifier: Apache-2.0
package main

// Placements: the overlay admins write over the estate file from the Tidy up page, so a
// tactical admin can put a system into its site and function, archive it, or bring it
// back, without waiting for a change to the estate file. The effective estate is the
// estate file plus this overlay (the overlay wins, and every changed system says who
// placed it). GET /api/estate/effective returns it in the estate file's own shape, so an
// admin can promote it into the repository's estate file by pull request; the overlay
// entries can then be dismissed. A placement in state "proposed" came with a member's
// feed request and takes no effect until an admin places it.

import (
	"context"
	"encoding/json"
	"errors"
	"time"

	"github.com/jackc/pgx/v5"
)

const placementSchema = `
CREATE TABLE IF NOT EXISTS placements (
  key       TEXT PRIMARY KEY,
  name      TEXT NOT NULL DEFAULT '',
  site      TEXT NOT NULL DEFAULT '',
  class     TEXT NOT NULL DEFAULT '',
  role      TEXT NOT NULL DEFAULT '',
  label     TEXT NOT NULL DEFAULT '',
  peer      TEXT NOT NULL DEFAULT '',
  status    TEXT NOT NULL DEFAULT '',
  archive   JSONB,
  far_side  JSONB,
  state     TEXT NOT NULL DEFAULT 'placed',
  by_who    TEXT NOT NULL,
  at        TIMESTAMPTZ NOT NULL,
  reason    TEXT NOT NULL DEFAULT ''
);
`

// Placement is one overlay entry. Key is the system's name in the estate (or, for a
// system not yet declared, the machine name it is declared under). Empty fields inherit
// from the estate file.
type Placement struct {
	Key     string       `json:"key"`
	Name    string       `json:"name,omitempty"` // a new machine name (rename); empty keeps Key
	Site    string       `json:"site,omitempty"`
	Class   string       `json:"class,omitempty"`
	Role    string       `json:"role,omitempty"`
	Label   string       `json:"label,omitempty"`
	Peer    string       `json:"peer,omitempty"`
	Status  string       `json:"status,omitempty"` // "" inherit | active | archived
	Archive *ArchiveInfo `json:"archive,omitempty"`
	FarSide *FarSide     `json:"far_side,omitempty"`
	State   string       `json:"state"` // placed | proposed
	By      string       `json:"by"`
	At      time.Time    `json:"at"`
	Reason  string       `json:"reason,omitempty"`
}

// Placements is what the server needs to keep the overlay.
type Placements interface {
	ListPlacements(ctx context.Context) ([]Placement, error)
	GetPlacement(ctx context.Context, key string) (Placement, error)
	// PutPlacement writes (or replaces) one placement and its audit event in one transaction.
	PutPlacement(ctx context.Context, p Placement, action, detail string) error
	DeletePlacement(ctx context.Context, key, actor, action, detail string) error
}

const placementCols = `key, name, site, class, role, label, peer, status, archive, far_side, state, by_who, at, reason`

func scanPlacement(row pgx.Row) (Placement, error) {
	var p Placement
	var arch, far []byte
	err := row.Scan(&p.Key, &p.Name, &p.Site, &p.Class, &p.Role, &p.Label, &p.Peer, &p.Status, &arch, &far, &p.State, &p.By, &p.At, &p.Reason)
	if errors.Is(err, pgx.ErrNoRows) {
		return p, ErrNotFound
	}
	if err != nil {
		return p, err
	}
	if len(arch) > 0 && string(arch) != "null" {
		p.Archive = &ArchiveInfo{}
		if err := json.Unmarshal(arch, p.Archive); err != nil {
			return p, err
		}
	}
	if len(far) > 0 && string(far) != "null" {
		p.FarSide = &FarSide{}
		if err := json.Unmarshal(far, p.FarSide); err != nil {
			return p, err
		}
	}
	return p, nil
}

func jsonOrNil(v any) any {
	switch t := v.(type) {
	case *ArchiveInfo:
		if t == nil {
			return nil
		}
	case *FarSide:
		if t == nil {
			return nil
		}
	}
	b, _ := json.Marshal(v)
	return string(b)
}

func (s *PGStore) ListPlacements(ctx context.Context) ([]Placement, error) {
	rows, err := s.pool.Query(ctx, `SELECT `+placementCols+` FROM placements ORDER BY at, key`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	out := []Placement{}
	for rows.Next() {
		p, err := scanPlacement(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, p)
	}
	return out, rows.Err()
}

func (s *PGStore) GetPlacement(ctx context.Context, key string) (Placement, error) {
	return scanPlacement(s.pool.QueryRow(ctx, `SELECT `+placementCols+` FROM placements WHERE key=$1`, key))
}

func (s *PGStore) PutPlacement(ctx context.Context, p Placement, action, detail string) error {
	tx, err := s.pool.Begin(ctx)
	if err != nil {
		return err
	}
	defer tx.Rollback(ctx) //nolint:errcheck
	_, err = tx.Exec(ctx, `INSERT INTO placements (`+placementCols+`) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9::jsonb,$10::jsonb,$11,$12,now(),$13)
ON CONFLICT (key) DO UPDATE SET name=$2, site=$3, class=$4, role=$5, label=$6, peer=$7, status=$8, archive=$9::jsonb,
  far_side=$10::jsonb, state=$11, by_who=$12, at=now(), reason=$13`,
		p.Key, p.Name, p.Site, p.Class, p.Role, p.Label, p.Peer, p.Status, jsonOrNil(p.Archive), jsonOrNil(p.FarSide), p.State, p.By, p.Reason)
	if err != nil {
		return err
	}
	if _, err = tx.Exec(ctx, `INSERT INTO feed_events (feed_id, at, actor, action, detail) VALUES ($1, now(), $2, $3, $4)`,
		"system:"+p.Key, p.By, action, detail); err != nil {
		return err
	}
	return tx.Commit(ctx)
}

func (s *PGStore) DeletePlacement(ctx context.Context, key, actor, action, detail string) error {
	tx, err := s.pool.Begin(ctx)
	if err != nil {
		return err
	}
	defer tx.Rollback(ctx) //nolint:errcheck
	tag, err := tx.Exec(ctx, `DELETE FROM placements WHERE key=$1`, key)
	if err != nil {
		return err
	}
	if tag.RowsAffected() == 0 {
		return ErrNotFound
	}
	if _, err = tx.Exec(ctx, `INSERT INTO feed_events (feed_id, at, actor, action, detail) VALUES ($1, now(), $2, $3, $4)`,
		"system:"+key, actor, action, detail); err != nil {
		return err
	}
	return tx.Commit(ctx)
}

// apply merges one placed entry into a system declaration.
func (p Placement) apply(base SystemDecl) SystemDecl {
	out := base
	if out.Name == "" {
		out.Name = p.Key
	}
	set := func(dst *string, v string) {
		if v != "" {
			*dst = v
		}
	}
	set(&out.Name, p.Name)
	set(&out.Site, p.Site)
	set(&out.Class, p.Class)
	set(&out.Role, p.Role)
	set(&out.Peer, p.Peer)
	if p.Label != "" {
		out.Label = p.Label
	} else if p.Name != "" && base.Label == base.Name {
		out.Label = p.Name
	}
	switch p.Status {
	case "archived":
		out.Status, out.Archive = "archived", p.Archive
	case "active":
		out.Status, out.Archive = "", nil
	}
	if p.FarSide != nil {
		out.FarSide = p.FarSide
	}
	out.PlacedBy = p.By
	return out
}

// WithOverlay returns the effective estate: this estate with every placed entry applied.
func (e *Estate) WithOverlay(ps []Placement) *Estate {
	if e == nil {
		e = newEstate()
	}
	n := newEstate()
	n.regionLabel, n.siteLabel, n.siteRegion, n.siteOrder = e.regionLabel, e.siteLabel, e.siteRegion, e.siteOrder
	n.fromFile, n.raw = e.fromFile, e.raw
	n.decls = append([]SystemDecl{}, e.decls...)
	for _, p := range ps {
		if p.State != "placed" {
			continue
		}
		idx := -1
		for i, d := range n.decls {
			if d.Name == p.Key {
				idx = i
				break
			}
		}
		if idx >= 0 {
			n.decls[idx] = p.apply(n.decls[idx])
			continue
		}
		d := p.apply(SystemDecl{})
		if d.Site == "" || n.siteLabel[d.Site] == "" {
			continue // a placement must name a declared site
		}
		n.decls = append(n.decls, d)
	}
	n.index()
	return n
}

// EffectiveJSON is the estate file with the overlay applied, in the file's own shape:
// every field the file carries that this service does not read (coordinates, parents,
// device types, sources) is kept as it was.
func (e *Estate) EffectiveJSON(ps []Placement) ([]byte, error) {
	doc := map[string]any{}
	if e != nil && len(e.raw) > 0 {
		if err := json.Unmarshal(e.raw, &doc); err != nil {
			return nil, err
		}
	}
	sites, _ := doc["sites"].([]any)
	siteMap := func(id string) map[string]any {
		for _, s := range sites {
			if m, ok := s.(map[string]any); ok && m["site_id"] == id {
				return m
			}
		}
		return nil
	}
	settings := map[string]SystemDecl{}
	if e != nil {
		for _, d := range e.decls {
			if !e.fromFile[d.Name] {
				settings[d.Name] = d
			}
		}
	}
	for _, p := range ps {
		if p.State != "placed" {
			continue
		}
		// Find (and take out) the system in the file, keeping every field it has.
		var sys map[string]any
		from := ""
		for _, s := range sites {
			m, ok := s.(map[string]any)
			if !ok {
				continue
			}
			list, _ := m["systems"].([]any)
			for i, x := range list {
				if xm, ok := x.(map[string]any); ok && xm["name"] == p.Key {
					sys, from = xm, m["site_id"].(string)
					m["systems"] = append(list[:i:i], list[i+1:]...)
					break
				}
			}
			if sys != nil {
				break
			}
		}
		if sys == nil {
			sys = map[string]any{"name": p.Key}
			if d, ok := settings[p.Key]; ok {
				b, _ := json.Marshal(d)
				_ = json.Unmarshal(b, &sys)
				from = d.Site
				delete(sys, "site")
			}
		}
		put := func(k, v string) {
			if v != "" {
				sys[k] = v
			}
		}
		put("name", p.Name)
		put("label", p.Label)
		put("class", p.Class)
		put("role", p.Role)
		put("peer", p.Peer)
		switch p.Status {
		case "archived":
			sys["status"], sys["archive"] = "archived", p.Archive
		case "active":
			delete(sys, "status")
			delete(sys, "archive")
		}
		if p.FarSide != nil {
			sys["far_side"] = p.FarSide
		}
		sys["placed_by"] = p.By
		target := p.Site
		if target == "" {
			target = from
		}
		sm := siteMap(target)
		if sm == nil {
			sm = siteMap(from)
		}
		if sm == nil {
			continue
		}
		list, _ := sm["systems"].([]any)
		sm["systems"] = append(list, sys)
	}
	if _, ok := doc["sites"]; !ok {
		doc["sites"] = []any{}
	}
	return json.MarshalIndent(doc, "", "  ")
}
