# dcontroller

Web Dante controller, so Dante control does not depend on a Windows PC with Dante Controller on the Dante network.

- All Dante protocol code lives in dstack. A protocol need or bug is a ticket in `zbynekdrlik/dstack`; this project only moves the stack version.
- Never change subscriptions or settings of a Dante device that is not ours without the owner's approval. Every change in the UI is an explicit, confirmed, logged action.
- Site facts stay out of this public repo.

## Playbook router
- Start here: `docs/HANDOVER.md`
