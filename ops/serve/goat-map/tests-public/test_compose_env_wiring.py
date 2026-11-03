# SPDX-License-Identifier: Apache-2.0
"""A variable a goatnet profile declares must be consumed by something.

WHY THIS EXISTS. A variable added to `goatmap/__main__.py` and a profile but NOT to
the compose `environment:` block passes every other check and never reaches the
process.

THE CONTRACT: `deploy/goatnets/<goatnet>.env` is the one file that carries
per-goatnet values (README, "Deploying"). A name declared there is a promise
that something reads it. Two things legitimately can — the compose file, by
interpolating it into a service, or `deploy.sh`, which renders the oauth2
fence from four of them. A name read by NEITHER is dead config that looks live,
which is worse than a missing variable because it silently disagrees with the
operator who set it.

Deliberately NOT asserted: that every `_env()` call in the app appears in a
profile. Several have working code defaults and some (`GOATMAP_WEB_ROOT`,
`GOATMAP_HTTP_PORT`) are baked into the Dockerfile. Requiring those would make
the guard noisy and it would be turned off.
"""
import glob
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
VAR = r"GOATMAP_[A-Z0-9_]+"


def _profile_vars() -> dict[str, set[str]]:
    out: dict[str, set[str]] = {}
    for f in sorted(glob.glob(str(ROOT / "deploy" / "goatnets" / "*.env"))) + [str(ROOT / ".env.example")]:
        out[Path(f).name] = set(re.findall(rf"^({VAR})=", Path(f).read_text(), re.M))
    return out


def _compose_consumed() -> set[str]:
    return set(re.findall(rf"\$\{{({VAR})", (ROOT / "docker-compose.yml").read_text()))


def _deploy_sh_consumed() -> set[str]:
    return set(re.findall(rf"({VAR})", (ROOT / "deploy" / "deploy.sh").read_text()))


def test_every_profile_var_is_consumed_by_compose_or_deploy_sh():
    consumed = _compose_consumed() | _deploy_sh_consumed()
    orphans = {n: sorted(v - consumed) for n, v in _profile_vars().items()}
    orphans = {n: v for n, v in orphans.items() if v}
    assert not orphans, (
        "these are declared in a goatnet profile but read by neither "
        f"docker-compose.yml nor deploy.sh, so setting them does nothing: {orphans}. "
        "Interpolate each into the compose service that needs it."
    )


def test_basemap_is_wired_and_has_no_default():
    """The specific regression, pinned: falling back to another goatnet's
    extract is the bug, not the safety net."""
    compose = (ROOT / "docker-compose.yml").read_text()
    m = re.search(r'GOATMAP_BASEMAP:\s*"\$\{GOATMAP_BASEMAP(:-[^}]*)?\}"', compose)
    assert m, "GOATMAP_BASEMAP is not interpolated in the compose environment block"
    assert m.group(1) in (None, ":-"), (
        "GOATMAP_BASEMAP must not default to a filename — an unset value must "
        "serve no basemap, never another environment's geography"
    )
    for name, declared in _profile_vars().items():
        assert "GOATMAP_BASEMAP" in declared, f"{name} declares no GOATMAP_BASEMAP"


def test_the_guard_can_see_both_sides():
    """A scan that matches nothing passes everything."""
    profiles = _profile_vars()
    assert len(profiles) >= 1, "expected at least .env.example"
    assert all(len(v) > 20 for v in profiles.values()), "profile scan collapsed"
    assert len(_compose_consumed()) > 20, "compose scan collapsed"
    assert "GOATMAP_OIDC_ISSUER_URL" in _deploy_sh_consumed(), "deploy.sh scan collapsed"
