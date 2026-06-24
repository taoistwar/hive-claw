#!/bin/bash
BASE_DIR=$(dirname "$0")
echo "BASE_DIR: $BASE_DIR"
HIVEWEB_DIR=$BASE_DIR/crates/hiveweb
echo "HIVEWEB_DIR: $HIVEWEB_DIR"
echo "Kill all hiveweb processes"
ps aux | grep hiveweb | grep -v grep|grep -v restart-hiveweb.sh | awk '{print $2}' | xargs kill -9
echo "hiveweb processes killed"
echo "Restart hiveweb"
cd $HIVEWEB_DIR
nohup cargo run --bin hiveweb > hiveweb.log 2>&1 &
echo "hiveweb restarted"
echo "Done"
