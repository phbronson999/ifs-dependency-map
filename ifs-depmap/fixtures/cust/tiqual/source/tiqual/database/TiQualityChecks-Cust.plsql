-----------------------------------------------------------------------------
--  Logical unit: TiQualityChecks
--  Component:    TIQUAL
-----------------------------------------------------------------------------
layer Cust;

-- Pure Cust package: it can only override what its own entity model generates,
-- so these have a base even though no file declares one.
@Override
PROCEDURE Check_Insert___ (
   newrec_ IN OUT ti_quality_checks_tab%ROWTYPE )
IS
BEGIN
   super(newrec_);
END Check_Insert___;

@Overtake Base
FUNCTION Get_Check_Type (
   check_id_ IN VARCHAR2 ) RETURN VARCHAR2
IS
BEGIN
   RETURN NULL;
END Get_Check_Type;
