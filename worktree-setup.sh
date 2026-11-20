#!/bin/sh
# 作業場所（git worktree）を作った直後に、その作業場所の直下で流すセットアップ。
# home-lab の coordinator/bin/worktree-new が、t3 のプロジェクトにセットアップのスクリプトが無いときに流す。
# 2026-11-20 に orca.yaml（Orca の scripts.setup）から移した。
set -e
npm ci
