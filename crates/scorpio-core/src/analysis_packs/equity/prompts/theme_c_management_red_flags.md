# Adapted from anthropics/financial-services (Apache 2.0) — equity-research/skills/earnings-analysis/SKILL.md, financial-analysis/skills/comps-analysis/SKILL.md

## Management Commentary Red Flags

When you see any of these in headlines, release summaries, or quoted commentary
present in fetched news data,
flag it explicitly in your output and give it weight in your assessment:

- "Macro headwinds" or "demand softness" language without specifics
- Customer concentration increasing or major customer loss
- Competitive intensity commentary ("pricing pressure", "share losses")
- Margin pressure or "investments" reducing near-term profitability
- Guidance pulled, reduced, or replaced with broader ranges
- Unusual one-time items inflating reported results
- Change in key operating metrics (churn, retention, win rates)

When a transcript is available (status: Found), compare the tone and
language between the press release / headline and the earnings call
segments. Flag any divergence — e.g., optimistic press release language
paired with cautious or evasive call commentary; CFO hedging on guidance
in the call that is absent from the headline; selective omission of
metrics in prepared remarks that the press release emphasizes.

When transcripts are unavailable (status: NotPublished / Throttled /
Unavailable), explicitly include the phrase
`degraded mode: transcript unavailable` in the affected summary.

## Data-Quality Red Flags (when reasoning over peer data)

- Inconsistent time periods (mixing quarterly and annual data)
- Negative-EBITDA companies valued on EBITDA multiples (use revenue instead)
- P/E ratios above 100x without an explicit hypergrowth narrative
- Mixing pure-play and conglomerate companies in the same comp set
- Different fiscal year ends without normalization
