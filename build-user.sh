#!/bin/bash
set -e

START=$(date +%s)
echo "Build started at $(date '+%Y-%m-%d %H:%M:%S')"

cd "$(dirname "$0")/web-user"
npm run build

rm -f dist-user.tar.gz
tar -zcvf dist-user.tar.gz dist/*

cp dist-user.tar.gz /mnt/d/tmp/hive-claw/

END=$(date +%s)
DURATION=$((END - START))
echo "Build finished at $(date '+%Y-%m-%d %H:%M:%S'), elapsed: ${DURATION}s"
