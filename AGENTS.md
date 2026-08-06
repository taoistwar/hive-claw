<!-- SPECKIT START -->
For additional context about technologies to be used, project structure,
shell commands, and other important information, read the current plan:
specs/004-agent-runtime/plan.md

Supporting design artifacts for the active feature:
- specs/004-agent-runtime/spec.md
- specs/004-agent-runtime/research.md
- specs/004-agent-runtime/data-model.md
- specs/004-agent-runtime/contracts/api.md
- specs/004-agent-runtime/contracts/host-functions.md
- specs/004-agent-runtime/quickstart.md

Constitution (always authoritative; cite, do not contradict):
.specify/memory/constitution.md (v1.4.0)

Product boundary (always authoritative): HiveWeb is the cloud-hosted Agent and
HiveGUI is the independent desktop-local Agent. They may reuse code and
contracts, but HiveGUI must never request or fall back to HiveWeb.
<!-- SPECKIT END -->
