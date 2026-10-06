# ADR-0002 Billing is the only module that writes invoices

Invoices are created, changed and stored only by billing. Other modules ask billing through
`BillingGateway`; they never touch the invoices table or `billing::persistence`.

Reason: invoices are legal documents; one writer keeps numbering, VAT and audit trail in one place.
