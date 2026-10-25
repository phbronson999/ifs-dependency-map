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

-- No such method in the Core package.
@Override
PROCEDURE Check_Update_Ti___ (
   newrec_ IN OUT shop_order_tab%ROWTYPE )
IS
BEGIN
   super(newrec_);
END Check_Update_Ti___;
