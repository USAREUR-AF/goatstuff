// SPDX-License-Identifier: Apache-2.0

// goat-feeds: self-service, approved delivery of fabric topics to mesh systems (ADR 1259).
// See docs/design/fabric-feeds.md in goatcore.
package main

import (
	"context"
	"flag"
	"fmt"
	"log"
	"net/http"
	"os"
	"os/signal"
	"strings"
	"syscall"
	"time"
)

var version = "dev"

func main() {
	cfgPath := flag.String("config", "/etc/goat-feeds/config.json", "site configuration")
	showVersion := flag.Bool("version", false, "print the version and exit")
	flag.Parse()
	if *showVersion {
		fmt.Println("goat-feeds", version)
		return
	}
	cfg, err := LoadConfig(*cfgPath)
	if err != nil {
		log.Fatalf("goat-feeds: %v", err)
	}
	dsn := os.Getenv("GOAT_FEEDS_DB_DSN")
	if dsn == "" && cfg.DBDSNFile != "" {
		b, err := os.ReadFile(cfg.DBDSNFile)
		if err != nil {
			log.Fatalf("goat-feeds: database DSN file: %v", err)
		}
		dsn = strings.TrimSpace(string(b))
	}
	if dsn == "" {
		log.Fatal("goat-feeds: no database: set db_dsn_file in the config or GOAT_FEEDS_DB_DSN")
	}
	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGINT, syscall.SIGTERM)
	defer stop()

	var reg *PGStore
	for attempt := 1; ; attempt++ {
		reg, err = OpenPG(ctx, dsn)
		if err == nil {
			break
		}
		if attempt >= 10 || ctx.Err() != nil {
			log.Fatalf("goat-feeds: database: %v", err)
		}
		log.Printf("goat-feeds: database not ready (%v); retrying", err)
		time.Sleep(3 * time.Second)
	}

	sup := NewSupervisor(cfg, reg, ExecSpawner, 3*time.Second)
	go sup.Run(ctx)

	decl := NewDeclared(cfg.DeclaredDir, cfg.DeclaredProbe.probe())
	go decl.Run(ctx, 10*time.Second)

	est, err := LoadEstate(cfg.EstateFile, cfg.Systems)
	if err != nil {
		log.Fatalf("goat-feeds: estate %s: %v", cfg.EstateFile, err)
	}

	srv := &Server{cfg: cfg, reg: reg, sup: sup, peers: NewPeers(cfg.Peers.API, cfg.Peers.TokenFile), decl: decl, est: est, start: time.Now(), pl: reg}
	if err := srv.reloadOverlay(ctx); err != nil {
		log.Fatalf("goat-feeds: placements: %v", err)
	}
	go srv.RunOverlay(ctx, 30*time.Second)
	hs := &http.Server{Addr: cfg.Listen, Handler: srv.Routes(), ReadHeaderTimeout: 10 * time.Second}
	go func() {
		<-ctx.Done()
		sc, cancel := context.WithTimeout(context.Background(), 8*time.Second)
		defer cancel()
		_ = hs.Shutdown(sc)
	}()
	log.Printf("goat-feeds %s: site %s, %d catalog topics, listening on %s", version, cfg.Site, len(cfg.Catalog), cfg.Listen)
	if err := hs.ListenAndServe(); err != nil && err != http.ErrServerClosed {
		log.Fatalf("goat-feeds: %v", err)
	}
	sup.StopAll()
}
