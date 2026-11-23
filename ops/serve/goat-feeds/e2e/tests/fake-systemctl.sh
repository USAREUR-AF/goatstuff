#!/bin/sh
# Stands in for `systemctl show <unit> -p ActiveState -p ActiveEnterTimestamp` in the browser tests.
echo "ActiveState=active"
echo "ActiveEnterTimestamp=Mon 2026-11-23 08:40:02 UTC"
