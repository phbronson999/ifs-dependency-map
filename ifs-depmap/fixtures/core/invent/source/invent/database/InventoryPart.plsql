-----------------------------------------------------------------------------
--  Logical unit: InventoryPart
--  Component:    INVENT
-----------------------------------------------------------------------------
layer Core;

@UncheckedAccess
FUNCTION Get_Description (
   contract_ IN VARCHAR2,
   part_no_  IN VARCHAR2 ) RETURN VARCHAR2
IS
   description_ VARCHAR2(200);
BEGIN
   SELECT description
      INTO  description_
      FROM  inventory_part_tab
      WHERE contract = contract_
      AND   part_no  = part_no_;
   RETURN description_;
END Get_Description;
