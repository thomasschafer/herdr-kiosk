# herdr-kiosk

A fuzzy repository and branch picker for Herdr. The current M1 scaffold displays a
placeholder popup; repository discovery arrives in later milestones.

## Development

`herdr plugin link` does not run manifest build commands. Run `just build` before
linking, or use `just link`, which builds the release binary first.
