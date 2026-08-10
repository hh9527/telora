# Intent Notes

The requested count and grouping map exactly to the public `OrdersCreated` measure and
`DeliveryException` dimension. `DeliveryException` is explicitly an unapproved capability, so the
public compiler is expected to reject this otherwise representable intent and report the policy
diagnostic rather than publish a plan.
