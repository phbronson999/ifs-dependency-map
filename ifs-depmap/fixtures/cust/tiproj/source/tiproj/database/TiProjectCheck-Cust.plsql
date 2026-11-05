-----------------------------------------------------------------------------
--  Logical unit: TiProjectCheck
--  Component:    TIPROJ
-----------------------------------------------------------------------------
layer Cust;

-- Every Project_API call here is guarded for PROJ, though PROJ is never the first or only
-- component of the nearest $IF (issue #23).
PROCEDURE Check_Project (
   project_id_ IN VARCHAR2 )
IS
BEGIN
   $IF Component_Quaman_SYS.INSTALLED AND Component_Proj_SYS.INSTALLED $THEN
      Project_API.Exist(project_id_);
   $END
   $IF Component_Proj_SYS.INSTALLED $THEN
      $IF Component_Quaman_SYS.INSTALLED $THEN
         Project_API.Exist(project_id_);
      $END
   $END
   $IF Component_Quaman_SYS.INSTALLED AND
       Component_Proj_SYS.INSTALLED $THEN
      Project_API.Exist(project_id_);
   $END
   $IF Component_Quaman_SYS.INSTALLED $THEN
      NULL;
   $ELSIF Component_Proj_SYS.INSTALLED $THEN
      Project_API.Exist(project_id_);
   $END
END Check_Project;

@DynamicComponentDependency PROJ
@DynamicComponentDependency QUAMAN
FUNCTION Get_Project_Name (
   project_id_ IN VARCHAR2 ) RETURN VARCHAR2
IS
BEGIN
   RETURN Project_API.Get_Name(project_id_);
END Get_Project_Name;
