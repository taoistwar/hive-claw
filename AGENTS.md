<!-- SPECKIT START -->
For additional context about technologies to be used, project structure,
shell commands, and other important information, read the current plan:
specs/011-hivegui-standalone-mode/plan.md

Supporting design artifacts for the active feature:
- specs/011-hivegui-standalone-mode/spec.md
- specs/011-hivegui-standalone-mode/research.md
- specs/011-hivegui-standalone-mode/data-model.md
- specs/011-hivegui-standalone-mode/contracts/api.md
- specs/011-hivegui-standalone-mode/contracts/host-functions.md
- specs/011-hivegui-standalone-mode/quickstart.md

Constitution (always authoritative; cite, do not contradict):
.specify/memory/constitution.md (v1.4.0)

Product boundary (always authoritative): HiveWeb is the cloud-hosted Agent and
HiveGUI is the independent desktop-local Agent. They may reuse code and
contracts, but HiveGUI must never request or fall back to HiveWeb.
<!-- SPECKIT END -->
