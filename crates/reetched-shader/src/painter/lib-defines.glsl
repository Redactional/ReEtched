// lib-defines.glsl: math constants and layer blending mode ids.
// Macros (not consts) so they also work in constant expressions and case labels.
#define M_PI 3.14159265
#define M_2PI (2.0 * M_PI)
#define M_INV_PI 0.31830988
#define M_INV_LOG2 1.442695
#define M_GOLDEN_RATIO 1.618034

#define BlendingMode_Disable 0
#define BlendingMode_Replace 1
#define BlendingMode_Normal 2
#define BlendingMode_Darken 3
#define BlendingMode_Multiply 4
#define BlendingMode_ColorBurn 5
#define BlendingMode_LinearBurn 6
#define BlendingMode_Lighten 7
#define BlendingMode_Screen 8
#define BlendingMode_ColorDodge 9
#define BlendingMode_LinearDodge 10
#define BlendingMode_Overlay 11
#define BlendingMode_SoftLight 12
#define BlendingMode_HardLight 13
#define BlendingMode_VividLight 14
#define BlendingMode_LinearLight 15
#define BlendingMode_PinLight 16
#define BlendingMode_Difference 17
#define BlendingMode_Exclusion 18
#define BlendingMode_Tint 19
#define BlendingMode_Saturation 20
#define BlendingMode_Color 21
#define BlendingMode_Value 22
#define BlendingMode_Divide 23
#define BlendingMode_InverseDivide 24
#define BlendingMode_Passthru 25
#define BlendingMode_NM_Combine 26
#define BlendingMode_NM_Oriented 27
#define BlendingMode_NM_InverseOriented 28
#define BlendingMode_Subtract 29
#define BlendingMode_InverseSubtract 30
#define BlendingMode_AddSub 31
