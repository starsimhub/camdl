#!/usr/bin/env bash
set -euo pipefail

CAMDLC=${CAMDLC:-camdlc}
CAMDL=${CAMDL:-camdl}
GOLDEN=ocaml/golden
PASS=0
FAIL=0

# Single temp file reused across iterations; cleaned up on any exit.
tmpir=$(mktemp /tmp/camdl_XXXXXX)
trap 'rm -f "$tmpir"' EXIT

for camdl in "$GOLDEN"/*.camdl; do
    name=$(basename "$camdl" .camdl)

    if ! "$CAMDLC" "$camdl" > "$tmpir"; then
        echo "FAIL [compile] $name"
        FAIL=$((FAIL + 1))
        continue
    fi

    # Prefer an explicit .params.toml; otherwise use the first scenario in the IR.
    if [ -f "$GOLDEN/$name.params.toml" ]; then
        params_flag="--params $GOLDEN/$name.params.toml"
    else
        first_scenario=$(python3 -c "
import json, sys
m = json.load(open('$tmpir'))
s = m.get('scenarios', [])
print(s[0]['name'] if s else '')
" 2>/dev/null || echo "")
        if [ -n "$first_scenario" ]; then
            params_flag="--scenario $first_scenario"
        else
            params_flag=""
        fi
    fi

    ok=1
    for backend in gillespie tau_leap chain_binomial; do
        tmperr=$(mktemp /tmp/camdl_err_XXXXXX)
        # shellcheck disable=SC2086
        if ! "$CAMDL" simulate "$tmpir" $params_flag --backend "$backend" --seed 42 > /dev/null 2>"$tmperr"; then
            if grep -q "requires capabilities" "$tmperr"; then
                # Expected: model needs features this backend doesn't support
                rm -f "$tmperr"
                continue
            fi
            echo "FAIL [$backend] $name"
            ok=0
            FAIL=$((FAIL + 1))
        fi
        rm -f "$tmperr"
    done

    if [ $ok -eq 1 ]; then
        echo "PASS $name"
        PASS=$((PASS + 1))
    fi
done

# ── Experiment pipeline tests ─────────────────────────────────────────────────

run_experiment_test() {
    local fixture="$1"        # e.g. tests/fixtures/exp_sir_basic.toml
    local expected_runs="$2"  # e.g. 50
    local name
    name=$(basename "$fixture" .toml)

    local outdir
    outdir=$(mktemp -d /tmp/camdl_exp_XXXXXX)
    trap "rm -rf '$outdir'" RETURN

    # run
    if ! "$CAMDL" experiment run "$fixture" --output-dir "$outdir" --parallel 2 > /dev/null; then
        echo "FAIL [experiment run] $name"; FAIL=$((FAIL+1)); return
    fi

    # check manifest completed count
    local completed
    completed=$(python3 -c "import json; m=json.load(open('$outdir/manifest.json')); print(m['completed'])")
    if [ "$completed" -ne "$expected_runs" ]; then
        echo "FAIL [manifest] $name: expected $expected_runs runs, got $completed"
        FAIL=$((FAIL+1)); return
    fi

    # resume is a no-op (re-run without --force, check it succeeds)
    if ! "$CAMDL" experiment run "$fixture" --output-dir "$outdir" --parallel 2 > /dev/null; then
        echo "FAIL [resume] $name"; FAIL=$((FAIL+1)); return
    fi

    # summarize
    if ! "$CAMDL" experiment summarize "$outdir" > /dev/null; then
        echo "FAIL [summarize] $name"; FAIL=$((FAIL+1)); return
    fi

    # check at least one scenario summary TSV exists
    if ! ls "$outdir"/analysis/summaries/*.tsv > /dev/null 2>&1; then
        echo "FAIL [summaries] $name: no summary TSVs found"
        FAIL=$((FAIL+1)); return
    fi

    echo "PASS [experiment] $name"
    PASS=$((PASS+1))
}

run_experiment_test tests/fixtures/exp_malaria.toml               60
run_experiment_test tests/fixtures/exp_sir_basic.toml             50
run_experiment_test tests/fixtures/exp_seir_erlang.toml           40
run_experiment_test tests/fixtures/exp_sir_five_age.toml          40
run_experiment_test tests/fixtures/exp_sir_patches_5.toml         40
run_experiment_test tests/fixtures/exp_seir_vaccine.toml          30
run_experiment_test tests/fixtures/exp_seir_vaccine_seasonal.toml 30
run_experiment_test tests/fixtures/exp_polio_spatial_5.toml       45

echo ""
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
