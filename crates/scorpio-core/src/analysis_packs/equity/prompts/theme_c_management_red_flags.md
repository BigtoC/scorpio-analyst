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

When transcripts are unavailable, explicitly include the phrase
`degraded mode: headline/summary only` in the affected summary.

<!-- TODO(transcripts): once call transcripts are wired (TranscriptEvidence
provider), add tone-shift detection between press release and earnings call.
Currently we only see press releases and headlines. -->

## Data-Quality Red Flags (when reasoning over peer data)

- Inconsistent time periods (mixing quarterly and annual data)
- Negative-EBITDA companies valued on EBITDA multiples (use revenue instead)
- P/E ratios above 100x without an explicit hypergrowth narrative
- Mixing pure-play and conglomerate companies in the same comp set
- Different fiscal year ends without normalization
