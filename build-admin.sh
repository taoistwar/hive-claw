#!/bin/bash
set -e

START=$(date +%s)
echo "Build started at $(date '+%Y-%m-%d %H:%M:%S')"

cd "$(dirname "$0")/web-admin"
npm run build

rm -f dist-admin.tar.gz
tar -zcvf dist-admin.tar.gz dist/*

cp dist-admin.tar.gz /mnt/d/tmp/hive-claw/

END=$(date +%s)
DURATION=$((END - START))
echo "Build finished at $(date '+%Y-%m-%d %H:%M:%S'), elapsed: ${DURATION}s"
