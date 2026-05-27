# WuWa Echo Sight

WuWa Echo Sight tracks Echo substat opening events and evaluates sequence-only predictions over those events. This context defines the domain language for prediction work.

## Language

**Prediction Accuracy**:
The primary success measure for sequence prediction: whether the model's first-ranked next substat matches the actual next opened substat. Optimize Top1 first; use probability calibration metrics as guardrails.
_Avoid_: Precision, accuracy, quality, EV

**Top1 Hit**:
A prediction outcome where the highest-ranked suggested substat equals the actual next opened substat.
_Avoid_: Correct guess, best pick

**Calibration Guardrail**:
A secondary check that predicted probabilities are not becoming overconfident while optimizing Top1. LogLoss is a guardrail, not the primary objective.
_Avoid_: Main score, primary accuracy

**Sequence Prediction**:
Prediction based on the ordered stream of opened substats and their tiers. It excludes Cost, main-stat conditioning, expected value, and continue/discard decisions.
_Avoid_: EV prediction, roll decision

## Example dialogue

Developer: "Did prediction accuracy improve?"
Domain expert: "Check Top1 first. If Top1 improves but LogLoss explodes, the model is overconfident and needs calibration before we trust it."

Developer: "Should expected substats change the model output?"
Domain expert: "No. Expectations can filter what the UI shows, but sequence prediction itself remains independent of EV or continue/discard logic."
