# Workflow action references and permissions

Built: external actions in the current workflows use full upstream commit SHAs. Version comments record the previously selected tag or branch. This change keeps the existing action major versions and build commands.

Built: workflows without explicit token defaults now start with `contents: read`. Existing job-level publishing permissions remain unchanged. Review token permissions when adding steps, especially reusable workflow callers.

Measured: parsed workflow comparison shows only action references and the added top-level permission defaults changed. Actionlint diagnostics match the current main baseline. Hosted image and release publication still require verification on their normal triggers; local lint does not prove publication.

Review action updates as code changes. A pin fixes the selected action revision; it does not pin every dependency downloaded by that action or by shell commands.
