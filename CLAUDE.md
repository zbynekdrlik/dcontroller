# dcontroller

Web Dante controller, so Dante control does not depend on a Windows PC with Dante Controller on the Dante network.

- All Dante protocol code lives in dstack. A protocol need or bug is a ticket in `zbynekdrlik/dstack`; this project only moves the stack version.
- Every change in the UI is an explicit, confirmed, logged action.
- A temporary test change of another Dante device's subscription or setting is the work: do it, log it on the ticket (device, what changed, times), and restore the original right after. Ask no approval question; the owner guards live events and stops the work himself.
- Never write "needs the owner's approval/decision" into a ticket without asking it in the same turn (❓) or labelling the ticket `needs-answer` / `needs-owner-action`.
- Site facts stay out of this public repo.

<!-- airuleset:tray=n/a server on the event box behind Cloudflare Access; users open it in a browser, no desktop user runs it -->

## Playbook router
- Start here: `docs/HANDOVER.md`
- E2E → `.claude/rules/e2e.md` (auto-loads on `e2e/**`)
- Web app → `.claude/rules/web.md` (auto-loads on `crates/web/**`)
