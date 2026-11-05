-----------------------------------------------------------------------------
--  Logical unit: TiProjectPart
--  Component:    TIPROJ
-----------------------------------------------------------------------------
layer Cust;

-- QUAMAN needs INVENT (Invent=STATIC in its deploy.ini), so a QUAMAN guard covers
-- Inventory_Part_API too (issue #24).
@DynamicComponentDependency QUAMAN
FUNCTION Get_Part_Description (
   contract_ IN VARCHAR2,
   part_no_  IN VARCHAR2 ) RETURN VARCHAR2
IS
BEGIN
   RETURN Inventory_Part_API.Get_Description(contract_, part_no_);
END Get_Part_Description;
