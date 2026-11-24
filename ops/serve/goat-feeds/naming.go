// SPDX-License-Identifier: Apache-2.0
package main

// The naming rule for systems (machine names), in one place. The request form, the
// tidy-up page and the API all call CheckName; GET /api/naming publishes the rule so
// other tools (the enrollment stage, scripts) can reuse it instead of copying it.

import (
	"fmt"
	"regexp"
	"sort"
	"strings"
)

// NameRule is the rule as people read it.
const NameRule = "<kind>-<site>-<function>-<nn>: lower case letters, digits and hyphens only. " +
	"kind is what the system is (console, radar, boundary); site is a declared site id; " +
	"function is its job (ops, cue, relay); nn is a two-digit number that tells systems of the same kind and job apart."

// NamePattern is the shape every name must have; CheckName adds the site check.
const NamePattern = `^[a-z0-9]+-[a-z0-9-]+-[a-z0-9]+(-[a-z0-9]+)*-[0-9]{2,3}$`

var (
	nameShapeRE = regexp.MustCompile(`^[a-z0-9]+(-[a-z0-9]+)*$`)
	nameNumRE   = regexp.MustCompile(`^[0-9]{2,3}$`)
	tokenBadRE  = regexp.MustCompile(`[^a-z0-9]+`)
)

// NameCheck is the result of checking one name.
type NameCheck struct {
	OK       bool     `json:"ok"`
	Problems []string `json:"problems"`
	Kind     string   `json:"kind,omitempty"`
	Site     string   `json:"site,omitempty"`
	Function string   `json:"function,omitempty"`
	Number   string   `json:"number,omitempty"`
}

// CheckName checks a machine name against the rule. sites are the declared site ids;
// site, when given, is the site the system is placed at and must match the name's.
func CheckName(name, site string, sites []string) NameCheck {
	c := NameCheck{Problems: []string{}}
	bad := func(format string, a ...any) NameCheck {
		c.Problems = append(c.Problems, fmt.Sprintf(format, a...))
		return c
	}
	if name == "" {
		return bad("Give the system a machine name, for example %s.", exampleName(sites))
	}
	if len(name) > 63 {
		return bad("Keep the name to 63 characters or fewer (one DNS label).")
	}
	if !nameShapeRE.MatchString(name) {
		return bad("Use only lower case letters, digits and single hyphens; %q has other characters, upper case, or a hyphen at an end.", name)
	}
	parts := strings.Split(name, "-")
	if len(parts) < 4 {
		return bad("A name has four parts, <kind>-<site>-<function>-<nn>; %q has %d.", name, len(parts))
	}
	c.Kind, c.Number = parts[0], parts[len(parts)-1]
	if !nameNumRE.MatchString(c.Number) {
		c.Problems = append(c.Problems, fmt.Sprintf("End the name with a two-digit number, for example -01; %q ends in %q.", name, c.Number))
	}
	middle := strings.Join(parts[1:len(parts)-1], "-")
	// The longest declared site id that starts the middle part, so site ids may hold hyphens.
	byLen := append([]string{}, sites...)
	sort.Slice(byLen, func(i, j int) bool { return len(byLen[i]) > len(byLen[j]) })
	for _, s := range byLen {
		if s == "" {
			continue
		}
		if middle == s {
			c.Site = s
			c.Problems = append(c.Problems, fmt.Sprintf("Put the system's function between the site and the number, for example %s-%s-ops-%s.", c.Kind, s, c.Number))
			break
		}
		if strings.HasPrefix(middle, s+"-") {
			c.Site, c.Function = s, strings.TrimPrefix(middle, s+"-")
			break
		}
	}
	if c.Site == "" {
		if len(sites) == 0 {
			c.Problems = append(c.Problems, "No sites are declared in the estate, so no name can be checked against a site.")
		} else {
			c.Problems = append(c.Problems, fmt.Sprintf("The second part must be a declared site id (%s); %q is not.", strings.Join(sites, ", "), parts[1]))
		}
	}
	if site != "" && c.Site != "" && c.Site != site {
		c.Problems = append(c.Problems, fmt.Sprintf("The name says site %q but the system is placed at site %q.", c.Site, site))
	}
	c.OK = len(c.Problems) == 0
	return c
}

// Token turns free text into a name part: lower case, runs of other characters become
// one hyphen.
func Token(s string) string {
	return strings.Trim(tokenBadRE.ReplaceAllString(strings.ToLower(strings.TrimSpace(s)), "-"), "-")
}

// SuggestName proposes the first free name for a kind, site and function.
func SuggestName(kind, site, function string, taken func(string) bool) string {
	kind, site, function = Token(kind), Token(site), Token(function)
	if kind == "" {
		kind = "system"
	}
	if function == "" {
		function = "ops"
	}
	if site == "" {
		site = "site"
	}
	for n := 1; n < 1000; n++ {
		name := fmt.Sprintf("%s-%s-%s-%02d", kind, site, function, n)
		if taken == nil || !taken(name) {
			return name
		}
	}
	return fmt.Sprintf("%s-%s-%s-01", kind, site, function)
}

func exampleName(sites []string) string {
	site := "north"
	if len(sites) > 0 {
		site = sites[0]
	}
	return "console-" + site + "-ops-01"
}
