-----------------------------------------------------------------------------
--  Logical unit: UserAllowedSite
--  Component:    FNDBAS
-----------------------------------------------------------------------------
layer Core;

@UncheckedAccess
PROCEDURE Exist (
   contract_ IN VARCHAR2 )
IS
BEGIN
   NULL;
END Exist;

@UncheckedAccess
FUNCTION Get_Default_Site RETURN VARCHAR2
IS
   site_ VARCHAR2(5);
BEGIN
   SELECT contract
      INTO  site_
      FROM  user_allowed_site_pub
      WHERE ROWNUM = 1;
   RETURN site_;
END Get_Default_Site;
