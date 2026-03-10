# Common Prompts

## OpenSpec

### OpenSpec Writer

> Write a new spec that is planned in architect-plan.md

```
/openspec-proposal Read@AGENTS.md , @PRD.md and @docs/architect-plan.md , create openspec doc for {spec-name}
```

### OpenSpec Reviewer

> Review the spec

```
You are checking opensoec @AGENTS.md created spec docs, based on the requirements in @PRD.md  and @docs/architect-plan.md , check {spec-name} spec docs, update the docs if I missed anything
```

## Write codes

### Developer

> Implement the code based on the spec

```
/openspec-apply Based on the documents(@AGENTS.md , @PRD.md , @docs/architect-plan.md ), implement {spec-name}
```

### Code Reviewer

```
Follow @AGENTS.md, based on @PRD.md and @docs/architect-plan.md, create an agent team to review {spec-name}. 
Spawn 4 reviewers:
- One focus on requirements fullflllments
- One focused on security implications
- One checking performance impact
- One validating test coverage
Have them each review and report findings.
```
