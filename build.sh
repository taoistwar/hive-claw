#!/bin/bash
set -e

TOTAL_START=$(date +%s)
SCRIPT_DIR="$(dirname "$0")"

if [ -z "${DEPLOY_DIR}" ]; then
    DEPLOY_DIR="${SCRIPT_DIR}/tmp"
fi
mkdir -p "${DEPLOY_DIR}"

echo "========================================"
echo "Build started at $(date '+%Y-%m-%d %H:%M:%S')"
echo "Deploy dir: ${DEPLOY_DIR}"
echo "========================================"

build_hiveweb() {
    echo ""
    echo "--- Building hiveweb (backend) ---"
    START=$(date +%s)
    cd "${SCRIPT_DIR}"
    cargo build -p hiveweb --release --target x86_64-unknown-linux-musl
    cp target/x86_64-unknown-linux-musl/release/hiveweb "${DEPLOY_DIR}/"
    END=$(date +%s)
    echo "hiveweb done, elapsed: $((END - START))s"
}

build_web_admin() {
    echo ""
    echo "--- Building web-admin (frontend) ---"
    START=$(date +%s)
    cd "${SCRIPT_DIR}/web-admin"
    npm run build
    rm -f dist-admin.tar.gz
    tar -zcvf dist-admin.tar.gz dist/*
    cp dist-admin.tar.gz "${DEPLOY_DIR}/"
    END=$(date +%s)
    echo "web-admin done, elapsed: $((END - START))s"
}

build_web_user() {
    echo ""
    echo "--- Building web-user (frontend) ---"
    START=$(date +%s)
    cd "${SCRIPT_DIR}/web-user"
    npm run build
    rm -f dist-user.tar.gz
    tar -zcvf dist-user.tar.gz dist/*
    cp dist-user.tar.gz "${DEPLOY_DIR}/"
    END=$(date +%s)
    echo "web-user done, elapsed: $((END - START))s"
}

# Default: build everything
case "${1:-all}" in
    hiveweb) build_hiveweb ;;
    admin)   build_web_admin ;;
    user)    build_web_user ;;
    frontend)
        build_web_admin
        build_web_user
        ;;
    all)
        build_hiveweb
        build_web_admin
        build_web_user
        ;;
    *)
        echo "Usage: $0 {hiveweb|admin|user|frontend|all}"
        exit 1
        ;;
esac

TOTAL_END=$(date +%s)
TOTAL_DURATION=$((TOTAL_END - TOTAL_START))
echo ""
echo "========================================"
echo "Build finished at $(date '+%Y-%m-%d %H:%M:%S'), total elapsed: ${TOTAL_DURATION}s"
echo "========================================"
