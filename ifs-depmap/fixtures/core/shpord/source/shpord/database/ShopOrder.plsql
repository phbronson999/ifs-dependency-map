-----------------------------------------------------------------------------
--  Logical unit: ShopOrder
--  Component:    SHPORD
-----------------------------------------------------------------------------
layer Core;

PROCEDURE Check_Insert___ (
   newrec_ IN OUT shop_order_tab%ROWTYPE )
IS
BEGIN
   User_Allowed_Site_API.Exist(newrec_.contract);
   $IF Component_Proj_SYS.INSTALLED $THEN
      IF newrec_.project_id IS NOT NULL THEN
         Project_API.Exist(newrec_.project_id);
      END IF;
   $END
END Check_Insert___;

PROCEDURE Release__ (
   order_no_ IN VARCHAR2 )
IS
   rec_ shop_order_tab%ROWTYPE;
BEGIN
   SELECT *
      INTO  rec_
      FROM  shop_order_tab
      WHERE order_no = order_no_;
   Check_Insert___(rec_);
   IF Get_Objstate(order_no_) = 'Planned' THEN
      UPDATE shop_order_tab
         SET   rowstate = 'Released'
         WHERE order_no = order_no_;
   ELSE
      Close__(order_no_);
   END IF;
END Release__;

-- Core overrides of methods generated from the entity model: the base is in no file,
-- and calling one from Core is not a dependency on Cust.
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

@Override
@UncheckedAccess
FUNCTION Get_Objstate (
   order_no_ IN VARCHAR2 ) RETURN VARCHAR2
IS
BEGIN
   RETURN super(order_no_);
END Get_Objstate;

@UncheckedAccess
FUNCTION Get_Part_Description (
   contract_ IN VARCHAR2,
   part_no_  IN VARCHAR2 ) RETURN VARCHAR2
IS
BEGIN
   RETURN Inventory_Part_API.Get_Description(contract_, part_no_);
END Get_Part_Description;

-- Reads the whole public record, so it evaluates every public derived attribute.
FUNCTION Get_Cost (
   order_no_ IN VARCHAR2 ) RETURN NUMBER
IS
   rec_ Shop_Order_API.Public_Rec;
BEGIN
   rec_ := Shop_Order_API.Get(order_no_);
   RETURN 0;
END Get_Cost;
