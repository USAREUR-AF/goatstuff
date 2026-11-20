# SPDX-License-Identifier: Apache-2.0
"""ops/gym/render.py turns every document the gym hosts into HTML and returns.

A line the block rules do not take (a table row with no header and separator
row above it, for one) still has to be consumed, or md_to_html never returns
and the site build hangs.
"""
import subprocess
import sys
import textwrap
from pathlib import Path

HERE = Path(__file__).resolve().parent


def render(text: str) -> str:
    """md_to_html in a child process, so a renderer that never returns fails the test."""
    prog = textwrap.dedent("""
        import sys
        sys.path.insert(0, sys.argv[1])
        import render
        sys.stdout.write(render.md_to_html(sys.stdin.read(), lambda target: target))
    """)
    done = subprocess.run([sys.executable, "-c", prog, str(HERE)], input=text,
                          capture_output=True, text=True, timeout=20)
    assert done.returncode == 0, done.stderr
    return done.stdout


def test_table_rows_without_a_header_are_rendered_and_the_renderer_returns():
    text = ("**Host keys too.** A paragraph before the rows.\n"
            "\n"
            "| **Sponsorship** | up to admin | removed |\n"
            "| **Group** | added | removed |\n"
            "\n"
            "After the rows.\n")
    html = render(text)
    assert "Sponsorship" in html and "Group" in html
    assert "After the rows." in html


def test_a_single_stray_row_at_the_end_is_rendered():
    html = render("Text.\n| only row |")
    assert "only row" in html


def test_a_table_with_its_separator_row_is_still_a_table():
    html = render("| a | b |\n|---|---|\n| 1 | 2 |\n")
    assert "<table>" in html and "<td>1</td>" in html
