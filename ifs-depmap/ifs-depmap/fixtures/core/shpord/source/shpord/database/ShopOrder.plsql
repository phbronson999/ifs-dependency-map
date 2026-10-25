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
   UPDATE shop_order_tab
      SET   rowstate = 'Released'
      WHERE order_no = order_no_;
END Release__;

@UncheckedAccess
FUNCTION Get_Part_Description (
   contract_ IN VARCHAR2,
   part_no_  IN VARCHAR2 ) RETURN VARCHAR2
IS
BEGIN
   RETURN Inventory_Part_API.Get_Description(contract_, part_no_);
END Get_Part_Description;
