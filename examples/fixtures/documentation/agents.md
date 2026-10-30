# Delegation and retained agents

This guide describes optional tools, not the capabilities enabled in a conversation.
Use task_status to recover actual task and agent IDs and availability.
Use agent_followup with the returned agent_id and a new task to continue an idle, available agent.
The same instance retains its conversation history; a follow-up creates a new task ID.
Use agent_send with agent_id and message to clarify a running mission.
A queued message is accepted for a future cycle, not proof that the model has read it.
If the instance has expired or is unavailable, delegate_task can create a new instance from a host-configured profile.
The host selects profiles, tools, expiry and the result-delivery policy.
A documentation read does not launch, resume or message an agent.
