// <auto-generated/> by Diplomat

#pragma warning disable 0105
using System;
using System.Runtime.InteropServices;

using DiplomatFeatures.Diplomat;
#pragma warning restore 0105

namespace DiplomatFeatures.Raw;

#nullable enable

[StructLayout(LayoutKind.Sequential)]
public partial struct NestedBorrowedFields
{
    private const string NativeLib = "diplomat_feature_tests";

    public BorrowedFields fields;

    public BorrowedFieldsWithBounds bounds;

    public BorrowedFieldsWithBounds bounds2;
}