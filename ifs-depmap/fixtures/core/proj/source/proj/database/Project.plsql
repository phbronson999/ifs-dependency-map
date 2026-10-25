-----------------------------------------------------------------------------
--  Logical unit: Project
--  Component:    PROJ
-----------------------------------------------------------------------------
layer Core;

PROCEDURE Exist (
   project_id_ IN VARCHAR2 )
IS
BEGIN
   NULL;
END Exist;

@UncheckedAccess
FUNCTION Get_Name (
   project_id_ IN VARCHAR2 ) RETURN VARCHAR2
IS
   name_ VARCHAR2(100);
BEGIN
   SELECT name INTO name_ FROM project_tab WHERE project_id = project_id_;
   RETURN name_;
END Get_Name;
