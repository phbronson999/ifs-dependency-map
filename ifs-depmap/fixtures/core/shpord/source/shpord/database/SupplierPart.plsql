-----------------------------------------------------------------------------
--  Logical unit: SupplierPart
--  Component:    SHPORD
-----------------------------------------------------------------------------
layer Core;

cache_vendor_no_ SUPPLIER_PART_TAB.vendor_no%TYPE;

-- Hand-written, and named like a getter, but not an attribute: the value comes from the cache.
FUNCTION Get_Primary_Vendor_No (
   part_no_ IN VARCHAR2 ) RETURN VARCHAR2
IS
BEGIN
   RETURN cache_vendor_no_;
END Get_Primary_Vendor_No;

-- An overload of the generated Get_Vendor_No(part_no_): two parameters, and a NUMBER.
FUNCTION Get_Vendor_No (
   part_no_  IN VARCHAR2,
   contract_ IN VARCHAR2 ) RETURN NUMBER
IS
BEGIN
   RETURN 1;
END Get_Vendor_No;
