-----------------------------------------------------------------------------
--  Logical unit: ShopOrder
--  Component:    SHPORD
-----------------------------------------------------------------------------
layer Cust;

@Override
PROCEDURE Check_Insert___ (
   newrec_ IN OUT shop_order_tab%ROWTYPE )
IS
BEGIN
   super(newrec_);
   -- PROJ is a DYNAMIC connection of SHPORD, but this call is not guarded.
   IF Project_API.Get_Name(newrec_.project_id) IS NULL THEN
      NULL;
   END IF;
END Check_Insert___;

-- The Core Insert___ is itself an override; this one layers over it.
@Override
PROCEDURE Insert___ (
   objid_      OUT    VARCHAR2,
   objversion_ OUT    VARCHAR2,
   newrec_     IN OUT shop_order_tab%ROWTYPE,
   attr_       IN OUT VARCHAR2 )
IS
BEGIN
   super(objid_, objversion_, newrec_, attr_);
END Insert___;

-- Not declared in Core either, but a template method the model generates.
@Override
PROCEDURE Check_Delete___ (
   remrec_ IN shop_order_tab%ROWTYPE )
IS
BEGIN
   super(remrec_);
END Check_Delete___;

-- A state event generated from the entity's state machine. Core calls it, so its
-- base exists below Core; it is still reported until .entity files are parsed.
@Override
PROCEDURE Close__ (
   order_no_ IN VARCHAR2 )
IS
BEGIN
   super(order_no_);
END Close__;

-- No such method in the Core package.
@Override
PROCEDURE Check_Update_Ti___ (
   newrec_ IN OUT shop_order_tab%ROWTYPE )
IS
BEGIN
   super(newrec_);
END Check_Update_Ti___;

FUNCTION Get_Ti_Vendor___ (
   order_no_ IN VARCHAR2 ) RETURN NUMBER
IS
   rec_   Shop_Order_API.Public_Rec;
   other_ Supplier_Part_API.Public_Rec;
BEGIN
   rec_ := Shop_Order_API.Get(order_no_);
   IF other_.primary_vendor IS NULL THEN
      RETURN rec_.primary_vendor;
   END IF;
   RETURN 0;
END Get_Ti_Vendor___;
