// SPDX-License-Identifier: Apache-2.0

package main

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"errors"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

const schema = `
CREATE TABLE IF NOT EXISTS feeds (
  id            TEXT PRIMARY KEY,
  topic_id      TEXT NOT NULL,
  output_id     TEXT NOT NULL,
  dest_name     TEXT NOT NULL,
  dest_ip       TEXT NOT NULL,
  dest_peer_id  TEXT NOT NULL DEFAULT '',
  port          INTEGER NOT NULL,
  transport     TEXT NOT NULL,
  purpose       TEXT NOT NULL DEFAULT '',
  owner         TEXT NOT NULL,
  status        TEXT NOT NULL,
  created_at    TIMESTAMPTZ NOT NULL,
  updated_at    TIMESTAMPTZ NOT NULL,
  decided_by    TEXT NOT NULL DEFAULT '',
  decided_at    TIMESTAMPTZ,
  decision_note TEXT NOT NULL DEFAULT '',
  end_at        TIMESTAMPTZ NOT NULL
);
CREATE TABLE IF NOT EXISTS feed_events (
  id       BIGSERIAL PRIMARY KEY,
  feed_id  TEXT NOT NULL,
  at       TIMESTAMPTZ NOT NULL,
  actor    TEXT NOT NULL,
  action   TEXT NOT NULL,
  detail   TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS feed_events_feed ON feed_events(feed_id, at);
CREATE INDEX IF NOT EXISTS feeds_status ON feeds(status);
ALTER TABLE feeds ADD COLUMN IF NOT EXISTS direction TEXT NOT NULL DEFAULT 'out';
ALTER TABLE feeds ADD COLUMN IF NOT EXISTS label TEXT NOT NULL DEFAULT '';
`

// Live states hold a destination port; the others release it.
var liveStates = []string{"pending", "active", "paused"}

type Feed struct {
	ID           string     `json:"id"`
	TopicID      string     `json:"topic_id"`
	OutputID     string     `json:"output_id"`
	DestName     string     `json:"dest_name"`
	DestIP       string     `json:"dest_ip"`
	DestPeerID   string     `json:"dest_peer_id"`
	Port         int        `json:"port"`
	Transport    string     `json:"transport"`
	Purpose      string     `json:"purpose"`
	Owner        string     `json:"owner"`
	Status       string     `json:"status"`
	CreatedAt    time.Time  `json:"created_at"`
	UpdatedAt    time.Time  `json:"updated_at"`
	DecidedBy    string     `json:"decided_by"`
	DecidedAt    *time.Time `json:"decided_at"`
	DecisionNote string     `json:"decision_note"`
	EndAt        time.Time  `json:"end_at"`
	// Direction "out" sends a topic to DestIP:Port. Direction "in" receives from the system
	// at DestIP on this core node's Port and publishes it as Label.
	Direction string `json:"direction"`
	Label     string `json:"label"`
}

type Event struct {
	ID     int64     `json:"id"`
	FeedID string    `json:"feed_id"`
	At     time.Time `json:"at"`
	Actor  string    `json:"actor"`
	Action string    `json:"action"`
	Detail string    `json:"detail"`
}

const feedCols = `id, topic_id, output_id, dest_name, dest_ip, dest_peer_id, port, transport, purpose, owner,
  status, created_at, updated_at, decided_by, decided_at, decision_note, end_at, direction, label`

// Registry is what the server and supervisor need from storage.
type Registry interface {
	Create(ctx context.Context, f Feed, actor, detail string) (Feed, error)
	Get(ctx context.Context, id string) (Feed, error)
	List(ctx context.Context, statuses ...string) ([]Feed, error)
	PortInUse(ctx context.Context, ip string, port int, transport string) (bool, error)
	ListenPortInUse(ctx context.Context, port int, transport string) (bool, error)
	Transition(ctx context.Context, id, status, actor, action, note string, decision bool, endAt *time.Time) (Feed, error)
	Events(ctx context.Context, feedID string, limit int) ([]Event, error)
	ExpireDue(ctx context.Context, now time.Time) ([]string, error)
	Ping(ctx context.Context) error
}

var ErrNotFound = errors.New("not found")

type PGStore struct{ pool *pgxpool.Pool }

func OpenPG(ctx context.Context, dsn string) (*PGStore, error) {
	pool, err := pgxpool.New(ctx, dsn)
	if err != nil {
		return nil, err
	}
	if _, err := pool.Exec(ctx, schema); err != nil {
		pool.Close()
		return nil, err
	}
	return &PGStore{pool: pool}, nil
}

func newID() string {
	b := make([]byte, 6)
	_, _ = rand.Read(b)
	return "feed-" + hex.EncodeToString(b)
}

func scanFeed(row pgx.Row) (Feed, error) {
	var f Feed
	err := row.Scan(&f.ID, &f.TopicID, &f.OutputID, &f.DestName, &f.DestIP, &f.DestPeerID, &f.Port, &f.Transport,
		&f.Purpose, &f.Owner, &f.Status, &f.CreatedAt, &f.UpdatedAt, &f.DecidedBy, &f.DecidedAt, &f.DecisionNote, &f.EndAt,
		&f.Direction, &f.Label)
	if errors.Is(err, pgx.ErrNoRows) {
		return f, ErrNotFound
	}
	return f, err
}

func (s *PGStore) Create(ctx context.Context, f Feed, actor, detail string) (Feed, error) {
	f.ID = newID()
	tx, err := s.pool.Begin(ctx)
	if err != nil {
		return Feed{}, err
	}
	defer tx.Rollback(ctx) //nolint:errcheck
	_, err = tx.Exec(ctx, `INSERT INTO feeds (`+feedCols+`) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,now(),now(),$12,$13,$14,$15,$16,$17)`,
		f.ID, f.TopicID, f.OutputID, f.DestName, f.DestIP, f.DestPeerID, f.Port, f.Transport, f.Purpose, f.Owner,
		f.Status, f.DecidedBy, f.DecidedAt, f.DecisionNote, f.EndAt, f.Direction, f.Label)
	if err != nil {
		return Feed{}, err
	}
	if _, err = tx.Exec(ctx, `INSERT INTO feed_events (feed_id, at, actor, action, detail) VALUES ($1, now(), $2, 'request', $3)`,
		f.ID, actor, detail); err != nil {
		return Feed{}, err
	}
	if f.Status == "active" {
		if _, err = tx.Exec(ctx, `INSERT INTO feed_events (feed_id, at, actor, action, detail) VALUES ($1, now(), $2, 'on', $3)`,
			f.ID, actor, f.DecisionNote); err != nil {
			return Feed{}, err
		}
	}
	if err = tx.Commit(ctx); err != nil {
		return Feed{}, err
	}
	return s.Get(ctx, f.ID)
}

func (s *PGStore) Get(ctx context.Context, id string) (Feed, error) {
	return scanFeed(s.pool.QueryRow(ctx, `SELECT `+feedCols+` FROM feeds WHERE id=$1`, id))
}

func (s *PGStore) List(ctx context.Context, statuses ...string) ([]Feed, error) {
	var rows pgx.Rows
	var err error
	if len(statuses) > 0 {
		rows, err = s.pool.Query(ctx, `SELECT `+feedCols+` FROM feeds WHERE status = ANY($1) ORDER BY created_at DESC`, statuses)
	} else {
		rows, err = s.pool.Query(ctx, `SELECT `+feedCols+` FROM feeds ORDER BY created_at DESC`)
	}
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	out := []Feed{}
	for rows.Next() {
		f, err := scanFeed(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, f)
	}
	return out, rows.Err()
}

func (s *PGStore) PortInUse(ctx context.Context, ip string, port int, transport string) (bool, error) {
	var n int
	err := s.pool.QueryRow(ctx, `SELECT count(*) FROM feeds WHERE direction='out' AND dest_ip=$1 AND port=$2 AND transport=$3 AND status = ANY($4)`,
		ip, port, transport, liveStates).Scan(&n)
	return n > 0, err
}

// ListenPortInUse: one receive port on the core node takes one live received feed.
func (s *PGStore) ListenPortInUse(ctx context.Context, port int, transport string) (bool, error) {
	var n int
	err := s.pool.QueryRow(ctx, `SELECT count(*) FROM feeds WHERE direction='in' AND port=$1 AND transport=$2 AND status = ANY($3)`,
		port, transport, liveStates).Scan(&n)
	return n > 0, err
}

func (s *PGStore) Transition(ctx context.Context, id, status, actor, action, note string, decision bool, endAt *time.Time) (Feed, error) {
	tx, err := s.pool.Begin(ctx)
	if err != nil {
		return Feed{}, err
	}
	defer tx.Rollback(ctx) //nolint:errcheck
	q := `UPDATE feeds SET status=$2, updated_at=now()`
	args := []any{id, status}
	if decision {
		q += `, decided_by=$3, decided_at=now(), decision_note=$4`
		args = append(args, actor, note)
	}
	if endAt != nil {
		args = append(args, *endAt)
		q += `, end_at=$` + itoa(len(args))
	}
	q += ` WHERE id=$1`
	tag, err := tx.Exec(ctx, q, args...)
	if err != nil {
		return Feed{}, err
	}
	if tag.RowsAffected() == 0 {
		return Feed{}, ErrNotFound
	}
	if _, err = tx.Exec(ctx, `INSERT INTO feed_events (feed_id, at, actor, action, detail) VALUES ($1, now(), $2, $3, $4)`,
		id, actor, action, note); err != nil {
		return Feed{}, err
	}
	if err = tx.Commit(ctx); err != nil {
		return Feed{}, err
	}
	return s.Get(ctx, id)
}

func (s *PGStore) Events(ctx context.Context, feedID string, limit int) ([]Event, error) {
	var rows pgx.Rows
	var err error
	if feedID != "" {
		rows, err = s.pool.Query(ctx, `SELECT id, feed_id, at, actor, action, detail FROM feed_events WHERE feed_id=$1 ORDER BY at DESC, id DESC LIMIT $2`, feedID, limit)
	} else {
		rows, err = s.pool.Query(ctx, `SELECT id, feed_id, at, actor, action, detail FROM feed_events ORDER BY at DESC, id DESC LIMIT $1`, limit)
	}
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	out := []Event{}
	for rows.Next() {
		var e Event
		if err := rows.Scan(&e.ID, &e.FeedID, &e.At, &e.Actor, &e.Action, &e.Detail); err != nil {
			return nil, err
		}
		out = append(out, e)
	}
	return out, rows.Err()
}

func (s *PGStore) ExpireDue(ctx context.Context, now time.Time) ([]string, error) {
	rows, err := s.pool.Query(ctx, `SELECT id FROM feeds WHERE status = ANY($1) AND end_at < $2`, liveStates, now)
	if err != nil {
		return nil, err
	}
	var ids []string
	for rows.Next() {
		var id string
		if err := rows.Scan(&id); err != nil {
			rows.Close()
			return nil, err
		}
		ids = append(ids, id)
	}
	rows.Close()
	for _, id := range ids {
		if _, err := s.Transition(ctx, id, "expired", "goat-feeds", "expire", "reached its end date", false, nil); err != nil {
			return ids, err
		}
	}
	return ids, nil
}

func (s *PGStore) Ping(ctx context.Context) error { return s.pool.Ping(ctx) }

func itoa(n int) string {
	if n < 10 {
		return string(rune('0' + n))
	}
	return itoa(n/10) + string(rune('0'+n%10))
}
