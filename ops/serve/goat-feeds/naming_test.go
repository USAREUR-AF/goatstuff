// SPDX-License-Identifier: Apache-2.0
package main

import (
	"strings"
	"testing"
)

func TestCheckName(t *testing.T) {
	sites := []string{"north", "harbor", "north-2"}
	cases := []struct {
		name, site string
		ok         bool
		want       string // a fragment of the first problem, or the parsed function when ok
	}{
		{"console-north-ops-01", "", true, "ops"},
		{"console-north-ops-01", "north", true, "ops"},
		{"radar-harbor-air-watch-12", "harbor", true, "air-watch"},
		{"boundary-north-2-relay-03", "north-2", true, "relay"},
		{"console-north-ops-123", "", true, "ops"},
		{"", "", false, "Give the system a machine name"},
		{"Console-north-ops-01", "", false, "lower case"},
		{"console_north_ops_01", "", false, "lower case"},
		{"console-north-ops-01-", "", false, "hyphen at an end"},
		{"console--north-ops-01", "", false, "single hyphens"},
		{"console-north-01", "", false, "four parts"},
		{"console-north-ops-x1", "", false, "two-digit number"},
		{"console-north-ops-1", "", false, "two-digit number"},
		{"console-south-ops-01", "", false, "declared site id"},
		{"console-north-ops-01", "harbor", false, "placed at site"},
		{"console-harbor-x-01", "north", false, "placed at site"},
		{strings.Repeat("a", 60) + "-north-ops-01", "", false, "63 characters"},
	}
	for _, c := range cases {
		got := CheckName(c.name, c.site, sites)
		if got.OK != c.ok {
			t.Errorf("CheckName(%q, %q).OK = %v, problems %v", c.name, c.site, got.OK, got.Problems)
			continue
		}
		if c.ok && got.Function != c.want {
			t.Errorf("CheckName(%q) function = %q, want %q", c.name, got.Function, c.want)
		}
		if !c.ok && (len(got.Problems) == 0 || !strings.Contains(got.Problems[0]+strings.Join(got.Problems, " "), c.want)) {
			t.Errorf("CheckName(%q, %q) problems %v, want one containing %q", c.name, c.site, got.Problems, c.want)
		}
	}
	// A site id that is the whole middle part means the function is missing.
	if got := CheckName("console-north-2-01", "", []string{"north-2"}); got.OK || !strings.Contains(strings.Join(got.Problems, " "), "function") {
		t.Errorf("missing function not named: %v", got.Problems)
	}
	if got := CheckName("console-north-ops-01", "", nil); got.OK {
		t.Errorf("no sites declared must not pass")
	}
}

func TestTokenAndSuggest(t *testing.T) {
	for in, want := range map[string]string{"Console": "console", "EFFECTOR-CUE": "effector-cue", " Air  watch! ": "air-watch", "": ""} {
		if got := Token(in); got != want {
			t.Errorf("Token(%q) = %q, want %q", in, got, want)
		}
	}
	taken := map[string]bool{"console-north-ops-01": true, "console-north-ops-02": true}
	if got := SuggestName("Console", "north", "OPS", func(n string) bool { return taken[n] }); got != "console-north-ops-03" {
		t.Errorf("SuggestName = %q", got)
	}
	if got := SuggestName("", "", "", nil); got != "system-site-ops-01" {
		t.Errorf("SuggestName defaults = %q", got)
	}
	if got := SuggestName("radar", "harbor", "watch", nil); !CheckName(got, "harbor", []string{"harbor"}).OK {
		t.Errorf("a suggestion must pass the rule: %q", got)
	}
}
