Making an AI app
* Create a rough PLAN.md sketch entailing a description of what you want to do. Keep it intentionally vague at first, outlining only the requirements that you need as a first pass.
* Instruct the agent to review the PLAN.md, and work with the user to formulate a well-structured plan for a new application.
* You should now be left with a pretty solid foundational PLAN.md document. Review this yourself and make sure it aligns with your goals.
* Close out of this agent, open a new one.
* Prompt an agent to become the "department head" for a new software application effort, and to task agents according to the work outlined in the PLAN.md document.
  * This ensures that this agent will delegate subtasks and run in parallel. One agent can work on frontend development, another on backend, a third on testing frameworks.
  * I recommend leaving this judgement of delegation to the agent, I had faster and more quality results from being hands off on telling the agent how to operate.
* Once PLAN.md has been fully implemented, instruct the agent to create a CLAUDE.md document at the top-level of the application root.
* Delete PLAN.md
* Claude should now generally know exactly how to interact with your application. You should be able to reproduce the "department head" workflow in a brand new session if desired.

Iterating on the AI app
* Create a TODO.md with the following header:
```
## TODO

--- DO NOT DELETE BELOW THIS LINE ---
Instructions:
* Review this document
* Determine how many agents are required to successfully complete the tasks
* Assign tasks to the agents accordingly
* Prompt agents for future improvements that could be made and note in this TODO.md document for the next cycle of iteration
--- CLEAN UP STATEMENTS BELOW THIS LINE WHICH HAVE BEEN COMPLETED SUCCESSFULLY ---
```
* Populate the TODO.md after the trailing comment with items you want the agents to work on
  * Keep these as concise bodies of work as possible.
  * Avoid a vague "rewrite this" - provide the agent with feedback on what you would like to see specifically changed
  * Be pedantic and overly descriptive. Agents can "see" but they don't quite have eyes like humans.
* Open an agent, prompt it to `become the "department head", review @CLAUDE.md and @TODO.md. Begin tasking agents with work after understanding the codebase`
* You should now have a fully iterative cycle going in your agent session. Continue to save items to TODO.md, and the background agents should continue to work on these items.
  * Think about this like the JIRA backlog. Task them in priority order. The most important item you want to see done should not be the last item on the TODO list! 
