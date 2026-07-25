# Agent notes

Full spec and step-by-step plan live in `spec.md` and `plan.md`. Work through
`plan.md` one step at a time, checking boxes off as you go, and record any
non-obvious decisions made along the way in its "Decisions made while
building" section.

## Verifying GUI/tray state

Don't screenshot the screen or scrape the clipboard to check the tray icon
title, menu bar, or other visual state — just ask what's currently shown.
It's faster and more reliable than any programmatic capture.
