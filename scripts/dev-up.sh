#!/usr/bin/env bash
set -euo pipefail

# Bring up MySQL, Redis, and MinIO for local development.
# Usage:
#   ./scripts/dev-up.sh          # start in foreground (Ctrl-C to stop)
#   ./scripts/dev-up.sh -d       # start in detached mode
#   ./scripts/dev-up.sh down     # stop and remove containers
#   ./scripts/dev-up.sh logs     # tail logs

cd "$(dirname "$0")"

case "${1:-up}" in
  down)
    docker compose down
    ;;
  logs)
    docker compose logs -f
    ;;
  ps)
    docker compose ps
    ;;
  -d|--detach)
    docker compose up -d
    echo
    echo "Services started:"
    echo "  MySQL:    mysql://hiveweb:hiveweb@127.0.0.1:13306/hiveweb"
    echo "  Redis:    redis://127.0.0.1:16379"
    echo "  MinIO:    http://127.0.0.1:19210   (console: http://127.0.0.1:19211, user=minioadmin pass=minioadmin)"
    ;;
  up|"")
    docker compose up
    ;;
  *)
    docker compose "$@"
    ;;
esac
