# Plans and revisions

Plan tools record declared work and progress. They do not execute the plan's steps.
Use plan_create to create a plan and plan_list or plan_read to recover its actual plan_id and revision.
Use plan_update with plan_id and expected_revision copied from the latest successful read or update.
A stale expected_revision is refused: reread the plan and reconcile the desired change.
When steps is supplied, it replaces the complete step list. Include unchanged steps and preserve stable step_id values.
Use plan_delete with the current expected_revision to delete a plan.
The host determines which plan tools and backend are enabled. MemoryPlanBackend lasts for its in-process lifetime.
This documentation demo enables only docs_list, docs_search and docs_read; no plan or delegation action can be executed here.
