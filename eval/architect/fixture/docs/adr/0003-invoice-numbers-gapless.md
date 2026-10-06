# ADR-0003 Invoice numbers are gapless and assigned by billing at finalization

The tax authority requires invoice numbers without gaps. Billing assigns the next number when an
invoice is finalized, inside the same transaction. No other module computes, reserves or passes in
an invoice number, and a draft has no number.
