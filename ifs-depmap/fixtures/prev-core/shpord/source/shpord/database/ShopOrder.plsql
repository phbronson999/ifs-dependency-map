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
END Release__;

@Override
PROCEDURE Insert___ (
   objid_      OUT    VARCHAR2,
   objversion_ OUT    VARCHAR2,
   newrec_     IN OUT shop_order_tab%ROWTYPE )
IS
BEGIN
   super(objid_, objversion_, newrec_);
END Insert___;
