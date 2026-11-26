#!/usr/bin/env bash
set -euo pipefail

RUBYCOM="${RUBYCOM:-./target/debug/rubyc}"
PASS=0
FAIL=0

for f in tests/*.rb; do
    echo "=== Testing $f ==="
    expected=$(ruby "$f")
    bin="/tmp/rubycom_test_$(basename "$f" .rb)"
    $RUBYCOM "$f" -o "$bin"
    actual=$("$bin")
    if [ "$expected" = "$actual" ]; then
        echo "PASS: $f"
        PASS=$((PASS + 1))
    else
        echo "FAIL: $f"
        diff -u <(echo "$expected") <(echo "$actual") || true
        FAIL=$((FAIL + 1))
    fi
done

echo ""
echo "Summary: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
